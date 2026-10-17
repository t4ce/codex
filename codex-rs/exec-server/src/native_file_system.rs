//! Filesystem-policy enforcement for TRUEOS's app-scoped, link-free namespace.
//!
//! TRUEOS cannot spawn a Linux/Seatbelt helper. Keep permission checks at the
//! executor boundary and use its normal filesystem APIs after each check. The
//! kernel independently confines these operations to the app's granted roots.

use std::io;

use codex_exec_server_protocol::JSONRPCErrorError;
use codex_protocol::permissions::ReadDenyMatcher;
use codex_utils_paths::path_uri::PathUri;

use crate::FileSystemSandboxContext;
use crate::fs_helper::FsHelperPayload;
use crate::fs_helper::FsHelperRequest;
use crate::fs_helper::map_fs_error;

fn denied(path: &PathUri) -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!("filesystem policy denies {path}"),
    )
}

pub(crate) fn check_path(
    sandbox: &FileSystemSandboxContext,
    path: &PathUri,
    write: bool,
) -> io::Result<()> {
    sandbox.validate_file_system_paths_for_current_host()?;
    let native = path.to_abs_path()?;
    let context = sandbox.policy_context();
    let policy = sandbox.permissions.file_system_sandbox_policy();
    let allowed = if write {
        policy.can_write_path(path, &context)
    } else {
        policy.can_read_path(path, &context)
            && !ReadDenyMatcher::from_context(&policy, &context)
                .is_some_and(|matcher| matcher.is_read_denied_uri(path, &context))
    };
    if !allowed {
        return Err(denied(path));
    }
    // The native kernel has no link nodes. Reject them explicitly as well, so
    // this adapter cannot accidentally acquire Unix alias semantics in tests
    // or when filesystem capabilities are extended later.
    for ancestor in native.ancestors() {
        match std::fs::symlink_metadata(ancestor.as_path()) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err(denied(path));
                }
                #[cfg(unix)]
                if write && metadata.is_file() {
                    use std::os::unix::fs::MetadataExt;
                    if metadata.nlink() > 1 {
                        return Err(denied(path));
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn check_directory_creation(sandbox: &FileSystemSandboxContext, path: &PathUri) -> io::Result<()> {
    check_path(sandbox, path, true)?;
    let native = path.to_abs_path()?;
    for parent in native.ancestors().skip(1) {
        match std::fs::symlink_metadata(parent.as_path()) {
            Ok(_) => break,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                check_path(sandbox, &PathUri::from_abs_path(&parent), true)?;
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn require_recursive_access(sandbox: &FileSystemSandboxContext, write: bool) -> io::Result<()> {
    let policy = sandbox.permissions.file_system_sandbox_policy();
    let context = sandbox.policy_context();
    let allowed = if write {
        policy.has_full_disk_write_access_with_context(&context)
    } else {
        policy.has_full_disk_read_access_for_convention(context.cwd.infer_path_convention())
            && !policy.has_denied_read_restrictions()
    };
    if allowed {
        Ok(())
    } else {
        // A single root check cannot authorize hidden descendants. Keep bulk
        // operations explicit until they have a per-entry policy walker.
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "recursive filesystem operation requires per-entry permission enforcement",
        ))
    }
}

pub(crate) async fn run(
    sandbox: &FileSystemSandboxContext,
    request: FsHelperRequest,
) -> Result<FsHelperPayload, JSONRPCErrorError> {
    validate(sandbox, &request).map_err(map_fs_error)?;
    crate::fs_helper::run_direct_request(request).await
}

fn validate(sandbox: &FileSystemSandboxContext, request: &FsHelperRequest) -> io::Result<()> {
    match request {
        FsHelperRequest::WriteFile(params) => {
            check_path(sandbox, &params.path, true)?;
            if params.expected_sha256.is_some() {
                check_path(sandbox, &params.path, false)?;
            }
            Ok(())
        }
        FsHelperRequest::CreateDirectory(params) => check_directory_creation(sandbox, &params.path),
        FsHelperRequest::ReadFile(params) => check_path(sandbox, &params.path, false),
        FsHelperRequest::GetMetadata(params) => check_path(sandbox, &params.path, false),
        FsHelperRequest::Canonicalize(params) => check_path(sandbox, &params.path, false),
        FsHelperRequest::ReadDirectory(params) => check_path(sandbox, &params.path, false),
        FsHelperRequest::Open(params) => check_path(
            sandbox,
            &params.path,
            params.mode == crate::protocol::FsOpenMode::Replace,
        ),
        FsHelperRequest::Walk(params) => {
            check_path(sandbox, &params.path, false)?;
            require_recursive_access(sandbox, false)
        }
        FsHelperRequest::Remove(params) => {
            check_path(sandbox, &params.path, true)?;
            let native = params.path.to_abs_path()?;
            if params.recursive.unwrap_or(true) && native.as_path().is_dir() {
                require_recursive_access(sandbox, true)?;
            }
            Ok(())
        }
        FsHelperRequest::Copy(params) => {
            check_path(sandbox, &params.source_path, false)?;
            check_path(sandbox, &params.destination_path, true)?;
            if params.recursive {
                require_recursive_access(sandbox, false)?;
                require_recursive_access(sandbox, true)?;
            }
            Ok(())
        }
        FsHelperRequest::LoadCapabilityDiscoveries { locations, .. } => {
            require_recursive_access(sandbox, false)?;
            for location in locations {
                check_path(sandbox, &location.root, false)?;
            }
            Ok(())
        }
        FsHelperRequest::DiscoverCapabilityRoots(params) => {
            require_recursive_access(sandbox, false)?;
            for root in &params.roots {
                check_path(sandbox, &root.path, false)?;
            }
            Ok(())
        }
    }
}

#[cfg(test)]
#[path = "native_file_system_tests.rs"]
mod tests;
