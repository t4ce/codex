use std::ffi::OsString;

pub(super) fn from_launch_environment(mut var_os: impl FnMut(&str) -> Option<OsString>) -> String {
    // The kernel supplies both names at Blueprint launch. Prefer its reserved
    // TRUEOS namespace, which Codex's dotenv loader cannot replace, rather than
    // asking the unavailable POSIX uname ABI for the same native identity.
    ["TRUEOS_HOSTNAME", "HOSTNAME"]
        .into_iter()
        .filter_map(|name| var_os(name))
        .map(|value| value.to_string_lossy().trim().to_owned())
        .find(|value| !value.is_empty())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::from_launch_environment;

    #[test]
    fn reserved_launch_hostname_takes_precedence() {
        assert_eq!(
            from_launch_environment(|name| match name {
                "TRUEOS_HOSTNAME" => Some("  native-machine  ".into()),
                "HOSTNAME" => Some("other-machine".into()),
                _ => None,
            }),
            "native-machine"
        );
    }

    #[test]
    fn blank_reserved_hostname_falls_back_to_launch_hostname() {
        assert_eq!(
            from_launch_environment(|name| match name {
                "TRUEOS_HOSTNAME" => Some(" \t ".into()),
                "HOSTNAME" => Some("  fallback-machine\n".into()),
                _ => None,
            }),
            "fallback-machine"
        );
    }

    #[test]
    fn unavailable_launch_identity_stays_empty() {
        assert_eq!(from_launch_environment(|_| None), "");
        assert_eq!(from_launch_environment(|_| Some(" \t ".into())), "");
    }
}
