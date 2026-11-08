//! Native CA artifacts live in the captured private workspace. Certificates
//! contain public data; their process-local signing key is never persisted.
use std::io;
use std::path::Path;

fn error(code: i32) -> io::Error {
    let kind = match code {
        -1 | -4 | -6 => io::ErrorKind::InvalidInput,
        -8 => io::ErrorKind::NotFound,
        -9 => io::ErrorKind::AlreadyExists,
        -19 => io::ErrorKind::PermissionDenied,
        -20 => io::ErrorKind::Unsupported,
        _ => io::ErrorKind::Other,
    };
    io::Error::new(
        kind,
        format!("native MITM CA storage operation failed ({code})"),
    )
}

pub(super) fn assert_private_path(path: &Path) -> io::Result<()> {
    trueos::async_fs::block_on(trueos::async_fs::assert_private_path(
        path.as_os_str().as_encoded_bytes(),
    ))
    .map_err(error)
}

pub(super) fn rename_without_replace(source: &Path, destination: &Path) -> io::Result<()> {
    trueos::async_fs::block_on(trueos::async_fs::rename(
        source.as_os_str().as_encoded_bytes(),
        destination.as_os_str().as_encoded_bytes(),
    ))
    .map_err(error)
}
