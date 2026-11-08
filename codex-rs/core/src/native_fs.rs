//! Native storage requirements for files containing session diagnostics.

use std::fs::File;
use std::fs::OpenOptions;
use std::io;
use std::path::Path;

/// Keep private diagnostics inside an owner-authorized native workspace.
/// Check before creating directories or truncating an existing file.
pub fn open_private_log(path: &Path, append: bool) -> io::Result<File> {
    trueos::async_fs::block_on(trueos::async_fs::assert_private_path(
        path.as_os_str().as_encoded_bytes(),
    ))
    .map_err(|code| {
        let kind = match code {
            -1 | -4 | -6 => io::ErrorKind::InvalidInput,
            -8 => io::ErrorKind::NotFound,
            -19 => io::ErrorKind::PermissionDenied,
            -20 => io::ErrorKind::Unsupported,
            _ => io::ErrorKind::Other,
        };
        io::Error::new(
            kind,
            format!("log path requires a native owner-authorized workspace ({code})"),
        )
    })?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    OpenOptions::new()
        .create(true)
        .write(true)
        .append(append)
        .truncate(!append)
        .open(path)
}
