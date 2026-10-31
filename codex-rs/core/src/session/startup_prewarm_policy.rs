//! Initial-session warmup policy; explicit idle warmup and turn transport are independent.

use std::ffi::OsStr;

pub(crate) const STARTUP_PREWARM_ENV: &str = "CDX_STARTUP_PREWARM";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PrewarmSchedule {
    Initial { websocket: bool },
    Explicit,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct InvalidStartupPrewarmOverride;

impl PrewarmSchedule {
    pub(crate) fn initial(
        trueos: bool,
        value: Option<&OsStr>,
    ) -> Result<Self, InvalidStartupPrewarmOverride> {
        if !trueos {
            return Ok(Self::Initial { websocket: true });
        }
        let websocket = match value {
            None => false,
            Some(value) if value == "0" => false,
            Some(value) if value == "1" => true,
            Some(_) => return Err(InvalidStartupPrewarmOverride),
        };
        Ok(Self::Initial { websocket })
    }

    pub(crate) fn websocket_enabled(self, provider_enabled: bool) -> bool {
        provider_enabled && !matches!(self, Self::Initial { websocket: false })
    }

    pub(crate) fn is_initial(self) -> bool {
        matches!(self, Self::Initial { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trueos_initial_warmup_defers_full_preparation_by_default() {
        for value in [None, Some(OsStr::new("0"))] {
            let policy = PrewarmSchedule::initial(true, value).unwrap();
            assert!(policy.is_initial());
            assert!(!policy.websocket_enabled(true));
        }
    }

    #[test]
    fn trueos_opt_in_allows_the_original_provider_gated_warmup() {
        let policy = PrewarmSchedule::initial(true, Some(OsStr::new("1"))).unwrap();
        assert!(policy.websocket_enabled(true));
        assert!(!policy.websocket_enabled(false));
    }

    #[test]
    fn invalid_overrides_are_rejected_without_retaining_the_value() {
        for value in ["", "true", "false", "01", " 1", "1 ", "2", "private-value"] {
            assert_eq!(
                PrewarmSchedule::initial(true, Some(OsStr::new(value))),
                Err(InvalidStartupPrewarmOverride),
            );
        }
        let oversized = "1".repeat(4096);
        assert_eq!(
            PrewarmSchedule::initial(true, Some(OsStr::new(&oversized))),
            Err(InvalidStartupPrewarmOverride),
        );
    }

    #[test]
    #[cfg(unix)]
    fn non_unicode_override_is_invalid() {
        use std::os::unix::ffi::OsStrExt;

        assert_eq!(
            PrewarmSchedule::initial(true, Some(OsStr::from_bytes(&[0xFF]))),
            Err(InvalidStartupPrewarmOverride),
        );
    }

    #[test]
    fn other_platforms_ignore_the_trueos_override() {
        for value in [None, Some(OsStr::new("0")), Some(OsStr::new("invalid"))] {
            let policy = PrewarmSchedule::initial(false, value).unwrap();
            assert!(policy.websocket_enabled(true));
            assert!(!policy.websocket_enabled(false));
        }
    }

    #[test]
    fn explicit_idle_warmup_keeps_its_original_provider_gate() {
        let policy = PrewarmSchedule::Explicit;
        assert!(!policy.is_initial());
        assert!(policy.websocket_enabled(true));
        assert!(!policy.websocket_enabled(false));
    }
}
