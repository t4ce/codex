//! The host-local rendezvous root for privileged app-server RPC sockets.
//!
//! Every listener uses this root, which sandboxes hide even before a daemon
//! starts. It must not depend on HOME, TMPDIR, CODEX_HOME, or command settings.

use std::fs;
use std::io;
#[cfg(not(target_os = "trueos"))]
use std::os::unix::fs::DirBuilderExt;
#[cfg(not(target_os = "trueos"))]
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;

/// Returns the fixed executor-local directory that every sandbox must hide.
pub fn shared_daemon_socket_directory() -> io::Result<PathBuf> {
    // Resolve the system alias /tmp -> /private/tmp on macOS.
    let temporary_root = fs::canonicalize("/tmp")?;
    let uid = unsafe { libc::geteuid() };
    Ok(temporary_root.join(format!("codex-daemon-{uid}")))
}

/// Creates the reserved directory, rejecting symlinks and unsafe existing owners or modes.
/// TRUEOS retains the directory check but bypasses POSIX owner/mode restrictions
/// while TRUEOSFS access-key enforcement is being integrated on the dev rig.
pub fn prepare_shared_daemon_socket_directory() -> io::Result<PathBuf> {
    let directory = shared_daemon_socket_directory()?;
    let mut builder = fs::DirBuilder::new();
    #[cfg(not(target_os = "trueos"))]
    builder.mode(0o700);
    match builder.create(&directory) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    let metadata = fs::symlink_metadata(&directory)?;
    let mut valid_directory = metadata.is_dir();
    #[cfg(not(target_os = "trueos"))]
    {
        valid_directory &=
            metadata.uid() == unsafe { libc::geteuid() } && metadata.mode() & 0o777 == 0o700;
    }
    if !valid_directory {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            #[cfg(not(target_os = "trueos"))]
            "app-server socket directory must be a user-owned directory with mode 0700",
            #[cfg(target_os = "trueos")]
            "app-server socket directory must be a directory",
        ));
    }
    Ok(directory)
}
