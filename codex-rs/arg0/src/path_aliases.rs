//! Native command sessions and in-process tools do not re-exec symlink aliases.
use std::ffi::OsString;
use std::path::PathBuf;

pub(super) fn prepare<T>(
    native: bool,
    package_path: Option<OsString>,
    existing_path: Option<OsString>,
    aliases: impl FnOnce(Option<OsString>) -> std::io::Result<(T, OsString)>,
) -> (Option<T>, Option<OsString>) {
    if native {
        return (None, package_path);
    }
    let path = package_path.clone().or(existing_path);
    match aliases(path) {
        Ok((guard, path)) => (Some(guard), Some(path)),
        Err(error) => {
            eprintln!("WARNING: proceeding, even though we could not create PATH aliases: {error}");
            (None, package_path)
        }
    }
}

pub(super) fn current_exe(
    native: bool,
    lookup: impl FnOnce() -> std::io::Result<PathBuf>,
) -> Option<PathBuf> {
    if native { None } else { lookup().ok() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_startup_does_not_invoke_alias_or_executable_lookup() {
        let (guard, path) = prepare::<()>(true, None, Some("native-path".into()), |_| {
            panic!("native startup must not create aliases")
        });
        assert!(guard.is_none());
        assert!(path.is_none());
        assert!(
            current_exe(true, || panic!(
                "native startup must not query executable path"
            ))
            .is_none()
        );
    }

    #[test]
    fn native_package_path_is_retained_without_materializing_aliases() {
        let (_, path) = prepare::<()>(
            true,
            Some("package:native".into()),
            Some("native".into()),
            |_| panic!("aliases"),
        );
        assert_eq!(path, Some("package:native".into()));
    }

    #[test]
    fn host_setup_calls_alias_provider_and_retains_its_guard_and_path() {
        let (guard, path) = prepare(
            false,
            Some("package:host".into()),
            Some("host".into()),
            |input| {
                assert_eq!(input, Some("package:host".into()));
                Ok((17, "aliases:package:host".into()))
            },
        );
        assert_eq!(guard, Some(17));
        assert_eq!(path, Some("aliases:package:host".into()));
        assert_eq!(
            current_exe(false, || Ok(PathBuf::from("/real/host/codex"))),
            Some(PathBuf::from("/real/host/codex"))
        );
    }

    #[test]
    fn host_failed_alias_creation_preserves_package_path() {
        let (guard, path) = prepare::<()>(
            false,
            Some("package:host".into()),
            Some("host".into()),
            |_| Err(std::io::ErrorKind::PermissionDenied.into()),
        );
        assert!(guard.is_none());
        assert_eq!(path, Some("package:host".into()));
    }
}
