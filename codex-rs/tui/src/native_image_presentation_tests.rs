use super::*;
use codex_transport::http::OutboundProxyPolicy;
use pretty_assertions::assert_eq;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

fn sources(markdown: &str, cwd: &Path) -> Vec<ImageSource> {
    NativeImagePresentationState::default()
        .take_images("thread", "turn", "message", markdown, cwd, false)
}

fn factory() -> HttpClientFactory {
    HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault)
}

fn image_bytes(format: ImageFormat) -> Vec<u8> {
    let image =
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(3, 2, image::Rgb([4, 80, 12])));
    let mut bytes = Cursor::new(Vec::new());
    image.write_to(&mut bytes, format).unwrap();
    bytes.into_inner()
}

#[test]
fn extracts_only_markdown_images_with_reference_and_literal_path_support() {
    let markdown = r#"![map](https://example.org/map%20of%20NY.png?signature=secret)
![local](<images/map with spaces.png>)
![absolute](/apps/common/map.png)
![file](file:///apps/common/map%20with%20spaces.png)
![reference][ny]

[ny]: https://example.org/manhattan.webp "Map"
[ordinary link](https://example.org/page.png)
`![inline code](https://example.org/code.png)`
\![escaped](https://example.org/escaped.png)
```md
![fenced code](https://example.org/fenced.png)
```
"#;
    let found: Vec<_> = sources(markdown, Path::new("/workspace"))
        .into_iter()
        .map(|source| source.destination)
        .collect();
    assert_eq!(
        found,
        vec![
            "https://example.org/map%20of%20NY.png?signature=secret",
            "images/map with spaces.png",
            "/apps/common/map.png",
            "file:///apps/common/map%20with%20spaces.png",
            "https://example.org/manhattan.webp",
        ]
    );
}

#[test]
fn completion_identity_and_replay_prevent_reopening_with_bounded_state() {
    let mut state = NativeImagePresentationState::default();
    let cwd = Path::new("/workspace");
    let image = "![map](map.png)";
    assert_eq!(state.take_images("a", "b", "c", image, cwd, false).len(), 1);
    assert!(
        state
            .take_images("a", "b", "c", image, cwd, false)
            .is_empty()
    );
    assert_eq!(
        state
            .take_images("other-thread", "b", "c", image, cwd, false)
            .len(),
        1
    );
    assert!(
        state
            .take_images("a", "b", "replayed", image, cwd, true)
            .is_empty()
    );
    assert!(
        state
            .take_images("a", "b", "replayed", image, cwd, false)
            .is_empty()
    );
    for n in 0..MAX_COMPLETED_ITEMS + 1 {
        state.take_images("a", "b", &n.to_string(), image, cwd, false);
    }
    assert_eq!(state.completed.len(), MAX_COMPLETED_ITEMS);
}

#[test]
fn repeated_destinations_are_taken_once_and_native_source_cap_applies() {
    let markdown = (0..MAX_IMAGES + 5)
        .map(|n| format!("![{n}](map{n}.png) ![again](map{n}.png)"))
        .collect::<Vec<_>>()
        .join("\n");
    let found = sources(&markdown, Path::new("/workspace"));
    assert_eq!(found.len(), MAX_IMAGES);
    assert_eq!(found.last().unwrap().destination, "map31.png");
}

#[tokio::test]
async fn local_sources_resolve_spaces_unicode_and_file_urls_and_are_retained() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let path = workspace.path().join("Mañhattan map.png");
    let original = image_bytes(ImageFormat::Png);
    std::fs::write(&path, &original).unwrap();
    let markdown = format!(
        "![relative](<Mañhattan map.png>)\n![encoded](Ma%C3%B1hattan%20map.png)\n![file]({})\n![absolute](<{}>)",
        Url::from_file_path(&path).unwrap(),
        path.display()
    );
    let result = present_images_with(
        sources(&markdown, workspace.path()),
        home.path().to_owned(),
        factory(),
        |paths| {
            assert!(!paths.is_empty());
            assert!(
                paths
                    .iter()
                    .all(|path| Path::new(path).starts_with(home.path().join("presented_images")))
            );
            Ok(())
        },
    )
    .await;
    assert_eq!(result.len(), 4);
    for result in result {
        assert!(result.error.is_none(), "{result:?}");
        assert_eq!(std::fs::read(result.path.unwrap()).unwrap(), original);
    }
    assert_eq!(std::fs::read(&path).unwrap(), original);
}

#[tokio::test]
async fn png_and_jpeg_keep_original_bytes_and_use_sniffed_extensions() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    for (format, suffix) in [(ImageFormat::Png, "png"), (ImageFormat::Jpeg, "jpg")] {
        let original = image_bytes(format);
        std::fs::write(workspace.path().join("wrong.extension"), &original).unwrap();
        let result = present_images_with(
            sources("![map](wrong.extension)", workspace.path()),
            home.path().to_owned(),
            factory(),
            |_| Ok(()),
        )
        .await;
        let path = result[0].path.as_ref().unwrap();
        assert_eq!(path.extension().unwrap(), suffix);
        assert_eq!(std::fs::read(path).unwrap(), original);
        assert!(result[0].error.is_none());
    }
}

#[tokio::test]
async fn embedded_gif_and_webp_are_normalized_to_native_png() {
    let home = tempfile::tempdir().unwrap();
    for format in [ImageFormat::Gif, ImageFormat::WebP] {
        let encoded = base64::engine::general_purpose::STANDARD.encode(image_bytes(format));
        let markdown = format!("![map](data:image/x-test;base64,{encoded})");
        let result = present_images_with(
            sources(&markdown, Path::new("/workspace")),
            home.path().to_owned(),
            factory(),
            |paths| {
                assert_eq!(paths.len(), 1);
                Ok(())
            },
        )
        .await;
        assert_eq!(result[0].source, "embedded image");
        assert!(result[0].error.is_none(), "{result:?}");
        let retained = std::fs::read(result[0].path.as_ref().unwrap()).unwrap();
        assert_eq!(image::guess_format(&retained).unwrap(), ImageFormat::Png);
        let decoded = image::load_from_memory(&retained).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (3, 2));
        assert!(!format!("{result:?}").contains(&encoded));
    }
}

#[tokio::test]
async fn real_http_download_follows_redirects_sniffs_content_and_sends_no_auth() {
    let home = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    let original = image_bytes(ImageFormat::Png);
    Mock::given(method("GET"))
        .and(path("/redirect"))
        .respond_with(ResponseTemplate::new(302).insert_header(
            "Location",
            format!("{}/not-an-image.html?signature=private", server.uri()),
        ))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/not-an-image.html"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(original.clone())
                .insert_header("Content-Type", "application/octet-stream"),
        )
        .expect(1)
        .mount(&server)
        .await;
    let markdown = format!("![Manhattan]({}/redirect?signature=private)", server.uri());
    let result = present_images_with(
        sources(&markdown, Path::new("/workspace")),
        home.path().to_owned(),
        factory(),
        |paths| {
            assert_eq!(paths.len(), 1);
            Ok(())
        },
    )
    .await;
    assert!(result[0].error.is_none(), "{result:?}");
    assert_eq!(
        std::fs::read(result[0].path.as_ref().unwrap()).unwrap(),
        original
    );
    assert_eq!(result[0].source, format!("{}/redirect", server.uri()));
    assert!(!format!("{result:?}").contains("private"));
    for request in server.received_requests().await.unwrap() {
        assert!(!request.headers.contains_key("authorization"));
        assert!(!request.headers.contains_key("cookie"));
    }
}

#[tokio::test]
async fn oversized_chunked_http_body_is_bounded_without_content_length() {
    use std::io::Read;
    let home = tempfile::tempdir().unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let _ = stream.read(&mut [0; 4096]);
        if stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
            )
            .is_err()
        {
            return;
        }
        let chunk = vec![0; 1024 * 1024];
        for _ in 0..33 {
            if stream
                .write_all(b"100000\r\n")
                .and_then(|_| stream.write_all(&chunk))
                .and_then(|_| stream.write_all(b"\r\n"))
                .is_err()
            {
                return;
            }
        }
        let _ = stream.write_all(b"0\r\n\r\n");
    });
    let markdown = format!("![large](http://{address}/large)");
    let result = present_images_with(
        sources(&markdown, Path::new("/workspace")),
        home.path().to_owned(),
        factory(),
        |_| panic!("oversized images must not be presented"),
    )
    .await;
    server.join().unwrap();
    assert_eq!(result[0].error.as_deref(), Some("image exceeds 32 MiB"));
    assert!(result[0].path.is_none());
    assert!(!home.path().join("presented_images").exists());
}

#[tokio::test]
async fn failures_are_per_image_and_native_admission_errors_keep_retained_paths() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(
        workspace.path().join("good.png"),
        image_bytes(ImageFormat::Png),
    )
    .unwrap();
    std::fs::write(workspace.path().join("html.png"), b"<html>no image</html>").unwrap();
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(404))
        .expect(1)
        .mount(&server)
        .await;
    let markdown = format!(
        "![bad](data:image/png;base64,bad)\n![missing](missing.png)\n![html](html.png)\n![HTTP]({}/secret?token=private)\n![good](good.png)",
        server.uri()
    );
    let result = present_images_with(
        sources(&markdown, workspace.path()),
        home.path().to_owned(),
        factory(),
        |paths| {
            assert_eq!(paths.len(), 1);
            Err(-16)
        },
    )
    .await;
    assert_eq!(result.len(), 5);
    assert!(
        result[..4]
            .iter()
            .all(|result| result.error.is_some() && result.path.is_none())
    );
    assert_eq!(
        result[3].error.as_deref(),
        Some("image download returned HTTP 404")
    );
    assert_eq!(
        result[4].error.as_deref(),
        Some("img admission failed (-16)")
    );
    assert!(result[4].path.as_ref().unwrap().is_file());
    assert!(!format!("{result:?}").contains("private"));
}

#[tokio::test]
async fn oversized_local_files_and_invalid_uri_schemes_are_not_presented() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let large = std::fs::File::create(workspace.path().join("large.png")).unwrap();
    large.set_len(MAX_SOURCE_BYTES as u64 + 1).unwrap();
    let markdown = "![large](large.png)\n![scheme](javascript:alert)\n![remote-file](file://other-host/map.png)\n![credentials](http://user:password@localhost/map.png)";
    let result = present_images_with(
        sources(markdown, workspace.path()),
        home.path().to_owned(),
        factory(),
        |_| panic!("no sources should be presented"),
    )
    .await;
    assert_eq!(result.len(), 4);
    assert_eq!(result[0].error.as_deref(), Some("image exceeds 32 MiB"));
    assert!(
        result
            .iter()
            .all(|result| result.path.is_none() && result.error.is_some())
    );
    assert!(!format!("{result:?}").contains("password"));
    assert!(!home.path().join("presented_images").exists());
}

#[test]
fn decoded_allocation_limit_rejects_oversized_headers_before_raster_allocation() {
    // GIF permits dimensions in its small fixed header, before any raster data.
    let bytes = b"GIF89a\xff\xff\xff\xff\x00\x00\x00;".to_vec();
    let home = tempfile::tempdir().unwrap();
    assert!(retain_native_image(bytes, home.path()).is_err());
    assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 0);
}
