//! Present images selected by a completed assistant reply in TRUEOS's native img.
//!
//! Markdown rendering remains side-effect free. Only live completion takes sources;
//! decoding and retained-file writes run away from the terminal event loop.

use base64::Engine;
use codex_transport::http::ClientRouteClass;
use codex_transport::http::HttpClient;
use codex_transport::http::HttpClientFactory;
use codex_transport::http::RouteAwareClientPool;
use image::ImageDecoder;
use image::ImageFormat;
use pulldown_cmark::Event;
use pulldown_cmark::Parser;
use pulldown_cmark::Tag;
use sha2::Digest;
use sha2::Sha256;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::io::Cursor;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use url::Url;

const MAX_IMAGES: usize = 32;
const MAX_COMPLETED_ITEMS: usize = 256;
const MAX_SOURCE_BYTES: usize = 32 * 1024 * 1024;
const MAX_DECODED_BYTES: u64 = 64 * 1024 * 1024;
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Default)]
pub(crate) struct NativeImagePresentationState {
    completed: VecDeque<(String, String, String)>,
}

pub(crate) struct ImageSource {
    destination: String,
    cwd: PathBuf,
}

#[derive(Debug)]
pub(crate) struct ImagePresentationResult {
    /// A display label; signed query strings and embedded bytes never reach status text.
    pub(crate) source: String,
    pub(crate) path: Option<PathBuf>,
    /// None means native queue admission succeeded, not that scanout was observed.
    pub(crate) error: Option<String>,
}

impl NativeImagePresentationState {
    pub(crate) fn take_images(
        &mut self,
        thread_id: &str,
        turn_id: &str,
        item_id: &str,
        markdown: &str,
        cwd: &Path,
        from_replay: bool,
    ) -> Vec<ImageSource> {
        let identity = (thread_id.to_owned(), turn_id.to_owned(), item_id.to_owned());
        if self.completed.contains(&identity) {
            return Vec::new();
        }
        // Remember replayed identities too: a subsequent duplicate notification
        // does not turn historical content into a request to open a window.
        self.completed.push_back(identity);
        if self.completed.len() > MAX_COMPLETED_ITEMS {
            self.completed.pop_front();
        }
        if from_replay {
            return Vec::new();
        }
        let mut seen = HashSet::new();
        Parser::new(markdown)
            .filter_map(|event| match event {
                Event::Start(Tag::Image { dest_url, .. })
                    if !dest_url.is_empty() && seen.insert(dest_url.to_string()) =>
                {
                    Some(ImageSource {
                        destination: dest_url.to_string(),
                        cwd: cwd.to_owned(),
                    })
                }
                _ => None,
            })
            .take(MAX_IMAGES)
            .collect()
    }
}

pub(crate) async fn present_images(
    sources: Vec<ImageSource>,
    presentation_root: PathBuf,
    http_client_factory: HttpClientFactory,
) -> Vec<ImagePresentationResult> {
    present_images_with(sources, presentation_root, http_client_factory, native_present).await
}

#[cfg(target_os = "trueos")]
fn native_present(paths: &[String]) -> Result<(), i32> {
    trueos_v::vshell::open_images(paths)
}

#[cfg(not(target_os = "trueos"))]
fn native_present(_paths: &[String]) -> Result<(), i32> {
    Err(-38)
}

async fn present_images_with(
    sources: Vec<ImageSource>,
    presentation_root: PathBuf,
    factory: HttpClientFactory,
    presenter: impl FnOnce(&[String]) -> Result<(), i32>,
) -> Vec<ImagePresentationResult> {
    // This pool retains the configured outbound routing and authorization policy,
    // with no OpenAI credentials, cookies, default headers or URL diagnostics.
    let client =
        RouteAwareClientPool::new_without_request_logging(factory, ClientRouteClass::Other)
            .into_client();
    let directory = presentation_root.join("presented_images");
    let mut results = Vec::new();
    for source in sources.into_iter().take(MAX_IMAGES) {
        let label = source_label(&source.destination);
        let materialized = match read_source(&source, &client).await {
            Ok(bytes) => {
                let directory = directory.clone();
                tokio::task::spawn_blocking(move || retain_native_image(bytes, &directory))
                    .await
                    .unwrap_or_else(|_| Err("image preparation worker failed".into()))
            }
            Err(error) => Err(error),
        };
        results.push(match materialized {
            Ok(path) => ImagePresentationResult {
                source: label,
                path: Some(path),
                error: None,
            },
            Err(error) => ImagePresentationResult {
                source: label,
                path: None,
                error: Some(error),
            },
        });
    }
    let paths: Vec<String> = results
        .iter()
        .filter_map(|result| result.path.as_ref())
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    if !paths.is_empty()
        && let Err(code) = presenter(&paths)
    {
        for result in results.iter_mut().filter(|result| result.path.is_some()) {
            result.error = Some(format!("img admission failed ({code})"));
        }
    }
    results
}

fn source_label(destination: &str) -> String {
    if destination.starts_with("data:") {
        return "embedded image".into();
    }
    if let Ok(mut url) = Url::parse(destination) {
        let _ = url.set_username("");
        let _ = url.set_password(None);
        url.set_query(None);
        url.set_fragment(None);
        return url.to_string();
    }
    destination.to_owned()
}

async fn read_source(source: &ImageSource, client: &HttpClient) -> Result<Vec<u8>, String> {
    let destination = &source.destination;
    if destination.starts_with("data:") {
        let (metadata, encoded) = destination
            .split_once(',')
            .ok_or("invalid embedded image")?;
        if !metadata.starts_with("data:image/") || !metadata.ends_with(";base64") {
            return Err("embedded image must use image base64 data".into());
        }
        // Reject oversized input before either percent decoding or base64 allocation.
        if encoded.len() > MAX_SOURCE_BYTES.saturating_mul(4).div_ceil(3) + 4 {
            return Err("image exceeds 32 MiB".into());
        }
        let encoded = urlencoding::decode(encoded).map_err(|_| "invalid embedded image")?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded.as_bytes())
            .map_err(|_| "invalid embedded image base64")?;
        return bounded(bytes);
    }
    let path = if let Ok(mut url) = Url::parse(destination) {
        match url.scheme() {
            "http" | "https" => {
                if !url.username().is_empty() || url.password().is_some() {
                    return Err("image URL must not contain credentials".into());
                }
                url.set_fragment(None);
                return tokio::time::timeout(DOWNLOAD_TIMEOUT, download(client, url))
                    .await
                    .map_err(|_| "image download timed out".to_owned())?;
            }
            "file" if url.host_str().is_none_or(|host| host == "localhost") => PathBuf::from(
                urlencoding::decode(url.path())
                    .map_err(|_| "invalid image file path")?
                    .as_ref(),
            ),
            _ => return Err("unsupported image source".into()),
        }
    } else {
        PathBuf::from(
            urlencoding::decode(destination)
                .map_err(|_| "invalid image file path")?
                .as_ref(),
        )
    };
    let path = if path.is_absolute() {
        path
    } else {
        source.cwd.join(path)
    };
    let metadata = tokio::fs::metadata(&path)
        .await
        .map_err(|_| "image file could not be read")?;
    if !metadata.is_file() {
        return Err("image source is not a regular file".into());
    }
    if metadata.len() > MAX_SOURCE_BYTES as u64 {
        return Err("image exceeds 32 MiB".into());
    }
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|_| "image file could not be read")?;
    let mut bytes = Vec::new();
    file.take(MAX_SOURCE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| "image file could not be read")?;
    bounded(bytes)
}

async fn download(client: &HttpClient, url: Url) -> Result<Vec<u8>, String> {
    let mut response = client
        .get(url)
        .timeout(DOWNLOAD_TIMEOUT)
        .send()
        .await
        .map_err(|_| "image download failed")?;
    if !response.status().is_success() {
        return Err(format!(
            "image download returned HTTP {}",
            response.status().as_u16()
        ));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_SOURCE_BYTES as u64)
    {
        return Err("image exceeds 32 MiB".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "image download failed")?
    {
        if bytes.len().saturating_add(chunk.len()) > MAX_SOURCE_BYTES {
            return Err("image exceeds 32 MiB".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn bounded(bytes: Vec<u8>) -> Result<Vec<u8>, String> {
    if bytes.len() > MAX_SOURCE_BYTES {
        Err("image exceeds 32 MiB".into())
    } else {
        Ok(bytes)
    }
}

fn retain_native_image(bytes: Vec<u8>, directory: &Path) -> Result<PathBuf, String> {
    let format = image::guess_format(&bytes).map_err(|_| "unrecognized image bytes")?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(MAX_DECODED_BYTES);
    let mut reader = image::ImageReader::with_format(Cursor::new(&bytes), format);
    reader.limits(limits.clone());
    let mut decoder = reader
        .into_decoder()
        .map_err(|_| "image decoder could not read image")?;
    let (width, height) = decoder.dimensions();
    if u64::from(width)
        .saturating_mul(u64::from(height))
        .saturating_mul(4)
        > MAX_DECODED_BYTES
    {
        return Err("decoded image exceeds 64 MiB".into());
    }
    limits
        .clone()
        .reserve(decoder.total_bytes())
        .map_err(|_| "decoded image exceeds 64 MiB")?;
    decoder
        .set_limits(limits)
        .map_err(|_| "image dimensions exceed decoder limits")?;
    let decoded =
        image::DynamicImage::from_decoder(decoder).map_err(|_| "image decoding failed")?;
    let (bytes, extension) = if matches!(format, ImageFormat::Png | ImageFormat::Jpeg) {
        (
            bytes,
            if format == ImageFormat::Png {
                "png"
            } else {
                "jpg"
            },
        )
    } else {
        let mut output = Cursor::new(Vec::new());
        decoded
            .write_to(&mut output, ImageFormat::Png)
            .map_err(|_| "image conversion to PNG failed")?;
        (output.into_inner(), "png")
    };
    std::fs::create_dir_all(directory)
        .map_err(|_| "image storage directory could not be created")?;
    let filename = format!("{:x}.{extension}", Sha256::digest(&bytes));
    let path = directory.join(filename);
    let mut file = tempfile::NamedTempFile::new_in(directory)
        .map_err(|_| "image storage could not be opened")?;
    file.write_all(&bytes)
        .map_err(|_| "image could not be stored")?;
    file.persist(&path)
        .map_err(|_| "image could not be retained")?;
    Ok(path)
}

#[cfg(test)]
#[path = "native_image_presentation_tests.rs"]
mod tests;
