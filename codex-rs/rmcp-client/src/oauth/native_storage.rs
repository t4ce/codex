//! OAuth persistence uses the captured native workspace key, never Unix modes.
use std::io;
use std::path::Path;

pub(super) fn assert_private_path(path: &Path) -> io::Result<()> {
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
            format!("MCP OAuth storage requires a native private workspace ({code})"),
        )
    })
}
