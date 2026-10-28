//! Shared native write boundary: callers keep permissions; storage keeps identity and atomic guards.

#[cfg(target_os = "trueos")]
use std::io;
#[cfg(target_os = "trueos")]
use std::path::Path;

/// Content identity for a new file is derived from its bytes, never its extension.
/// Empty UTF-8 text needs an explicit identity because the detector has no evidence.
fn new_content_type(contents: &[u8]) -> infer::ContentTypeId {
    infer::detect_content_type(contents).unwrap_or_else(|| {
        if std::str::from_utf8(contents).is_ok() {
            infer::ContentTypeId::UTF8_TEXT
        } else {
            infer::ContentTypeId::BLOB
        }
    })
}

#[cfg(target_os = "trueos")]
fn native_path(path: &Path) -> io::Result<&[u8]> {
    path.to_str()
        .map(str::as_bytes)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "TRUEOS paths must be UTF-8"))
}

#[cfg(target_os = "trueos")]
fn native_error(code: i32) -> io::Error {
    use trueos_v::vfs_async as fs;
    let kind = match code {
        fs::ERR_BAD_UTF8 | fs::ERR_BAD_PARAM | -6 | -7 => io::ErrorKind::InvalidInput,
        fs::ERR_NOT_FOUND => io::ErrorKind::NotFound,
        fs::ERR_ALREADY_EXISTS => io::ErrorKind::AlreadyExists,
        fs::ERR_TYPE_REQUIRED | fs::ERR_TYPE_MISMATCH => io::ErrorKind::InvalidData,
        fs::ERR_WRITE_CONFLICT => io::ErrorKind::WouldBlock,
        -14 => io::ErrorKind::TimedOut,
        -3 => return io::Error::from_raw_os_error(28),
        _ => io::ErrorKind::Other,
    };
    let message = if code == fs::ERR_WRITE_CONFLICT {
        "file changed since the expected SHA-256; read it again before replacing".to_string()
    } else {
        format!("native TRUEOS filesystem error {code}")
    };
    io::Error::new(kind, message)
}

#[cfg(target_os = "trueos")]
pub(crate) async fn write_file(
    path: &Path,
    contents: &[u8],
    options: crate::WriteFileOptions,
) -> io::Result<()> {
    // The kernel chooses the current stored type under its namespace write lease;
    // the inferred type applies only when this mutation creates a missing file.
    trueos_v::vfs_async::write_file_typed_checked(
        native_path(path)?,
        contents,
        new_content_type(contents),
        options.create_new,
        options.expected_sha256,
    )
    .await
    .map_err(native_error)
}

#[cfg(target_os = "trueos")]
pub(crate) async fn content_type(path: &Path) -> io::Result<Option<u32>> {
    let metadata = trueos_v::vfs_async::typed_metadata(native_path(path)?)
        .await
        .map_err(native_error)?;
    Ok((metadata.kind == trueos_v::vfs_async::NodeKind::File)
        .then_some(metadata.content_type.raw()))
}

#[cfg(target_os = "trueos")]
pub(crate) async fn read_directory(path: &Path) -> io::Result<Vec<crate::ReadDirectoryEntry>> {
    let listing = trueos_v::vfs_async::list_dir_typed(native_path(path)?)
        .await
        .map_err(native_error)?;
    if listing.truncated {
        return Err(io::Error::other(
            "TRUEOS directory listing was truncated; complete enumeration is unavailable",
        ));
    }

    Ok(listing
        .entries
        .into_iter()
        .map(|entry| crate::ReadDirectoryEntry {
            file_name: entry.name,
            is_directory: entry.kind == trueos_v::vfs_async::NodeKind::Directory,
            is_file: entry.kind == trueos_v::vfs_async::NodeKind::File,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::new_content_type;
    use infer::ContentTypeId;

    #[test]
    fn text_has_no_extension_whitelist_or_newline_changes() {
        for contents in [
            b"".as_slice(),
            b"hello codex",
            b"fn main() {}\n",
            b"{\"id\":1}",
        ] {
            assert_eq!(new_content_type(contents), ContentTypeId::UTF8_TEXT);
        }
    }

    #[test]
    fn registered_signatures_and_binary_content_keep_their_identity() {
        assert_eq!(
            new_content_type(b"<!doctype html>\n<html><body>hello</body></html>\n"),
            ContentTypeId::HTML,
        );
        assert_eq!(new_content_type(&[0xff, 0xfe, 0xfd]), ContentTypeId::BLOB);
    }
}
