/// The current Codex CLI version as embedded at compile time.
pub const CODEX_CLI_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Identity shown in the title. TRUEOS identifies the loaded Blueprint package
/// rather than the workspace's placeholder Cargo version.
pub(crate) fn display_version(_version: &str) -> String {
    #[cfg(target_os = "trueos")]
    {
        static PACKAGE_VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        PACKAGE_VERSION
            .get_or_init(|| {
                let hash = std::env::var("TRUEOS_APP_SHA256").ok();
                packed_app_display_version(hash.as_deref())
            })
            .clone()
    }
    #[cfg(not(target_os = "trueos"))]
    {
        format!("v{_version}")
    }
}

#[cfg(any(target_os = "trueos", test))]
fn packed_app_display_version(hash: Option<&str>) -> String {
    match hash.filter(|hash| hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        Some(hash) => hash[..8].to_ascii_lowercase(),
        None => "package unknown".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_app_title_uses_the_first_eight_sha256_digits() {
        let hash = "37F6EB1A5573BC2DD63A5B6C5530F9423323F2C62AD333E130ECE1070659C6AC";
        assert_eq!(packed_app_display_version(Some(hash)), "37f6eb1a");
    }

    #[test]
    fn missing_or_invalid_package_identity_is_explicit() {
        for hash in [None, Some(""), Some("37f6eb1a"), Some("not a package hash")] {
            assert_eq!(packed_app_display_version(hash), "package unknown");
        }
        assert_eq!(
            packed_app_display_version(Some(&"z".repeat(64))),
            "package unknown"
        );
    }
}
