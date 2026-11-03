//! Browser handoff uses Hull's launch-script protocol on TRUEOS.

#[cfg(not(target_os = "trueos"))]
pub use webbrowser::open as open_browser;

/// Open a page in an independent Hull shell, leaving the caller's terminal intact.
#[cfg(target_os = "trueos")]
pub fn open_browser(target: &str) -> std::io::Result<()> {
    let script = hull_launch_script(target)?;
    trueos_v::vshell::launch_with_destination(
        "hull",
        &script,
        trueos_v::vshell::LaunchDestination::NewShell3,
    )
    .map_err(|code| std::io::Error::other(format!("Could not launch Hull ({code})")))
}

#[cfg(any(test, target_os = "trueos"))]
fn hull_launch_script(target: &str) -> std::io::Result<String> {
    use std::io::Error;
    use std::io::ErrorKind;

    // Hull accepts one bounded URL per line. Validate before parsing so URL
    // normalization cannot hide control characters or additional commands.
    if target.len() > 4096 || target.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(Error::new(ErrorKind::InvalidInput, "Invalid Hull URL"));
    }
    let url = url::Url::parse(target)
        .map_err(|_| Error::new(ErrorKind::InvalidInput, "Invalid Hull URL"))?;
    // Match Hull's public-page and local HTML preview URL policy.
    if url.as_str().len() > 4096
        || !url.username().is_empty()
        || url.password().is_some()
        || url.host_str().is_none()
        || !(url.scheme() == "https"
            || (url.scheme() == "http"
                && url.host_str() == Some("127.0.0.1")
                && url.port().is_some_and(|port| port != 0)))
    {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "Hull requires an HTTPS or loopback preview URL",
        ));
    }
    Ok(format!("open {url}\n"))
}

#[cfg(test)]
mod tests {
    use super::hull_launch_script;
    use std::io::ErrorKind;

    #[test]
    fn usage_link_becomes_one_hull_command() {
        assert_eq!(
            hull_launch_script("https://chatgpt.com/settings/usage").unwrap(),
            "open https://chatgpt.com/settings/usage\n"
        );
    }

    #[test]
    fn authorization_query_and_fragment_are_preserved() {
        let url = "https://auth.example/authorize?redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback&state=a%2Bb%3D#continue";
        assert_eq!(hull_launch_script(url).unwrap(), format!("open {url}\n"));
    }

    #[test]
    fn local_preview_is_supported() {
        let url = "http://127.0.0.1:49180/index.html";
        assert_eq!(hull_launch_script(url).unwrap(), format!("open {url}\n"));
    }

    #[test]
    fn unsupported_targets_and_script_injection_are_rejected() {
        for target in [
            "",
            "https://",
            "file:///tmp/page.html",
            "javascript:alert(1)",
            "http://example.com/",
            "http://localhost:49180/",
            "http://127.0.0.1/",
            "http://127.0.0.1:0/",
            "https://user:password@example.com/",
            "https://example.com/\nopen https://other.example/",
            "https://example.com/\r",
            "https://example.com/\0",
            "https://example.com/\t",
            "https://example.com/\u{1b}",
            "https://example.com/a b",
        ] {
            assert_eq!(
                hull_launch_script(target).unwrap_err().kind(),
                ErrorKind::InvalidInput
            );
        }
    }

    #[test]
    fn oversized_and_normalization_expanded_urls_are_rejected() {
        for suffix in ["x".repeat(4096), "é".repeat(1000)] {
            assert!(hull_launch_script(&format!("https://example.com/{suffix}")).is_err());
        }
    }
}
