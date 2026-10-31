//! Coalesce the first automatic session skills refresh with startup's background request.

use std::ffi::OsStr;

use super::*;

const STARTUP_SKILLS_COALESCE_ENV: &str = "CDX_STARTUP_SKILLS_COALESCE";

fn startup_skills_coalesce_enabled(
    trueos: bool,
    value: Option<&OsStr>,
) -> std::result::Result<bool, ()> {
    if !trueos {
        return Ok(false);
    }
    match value {
        None => Ok(true),
        Some(value) if value == "1" => Ok(true),
        Some(value) if value == "0" => Ok(false),
        Some(_) => Err(()),
    }
}

impl App {
    pub(super) fn arm_startup_skills_refresh(&mut self) {
        if !cfg!(target_os = "trueos") {
            return;
        }
        let value = std::env::var_os(STARTUP_SKILLS_COALESCE_ENV);
        let enabled = startup_skills_coalesce_enabled(true, value.as_deref()).unwrap_or_else(|()| {
            codex_arg0::startup_timing::mark("tui_startup_skills_override_invalid");
            tracing::warn!("invalid CDX_STARTUP_SKILLS_COALESCE; expected 0 or 1; startup skills coalescing enabled");
            true
        });
        codex_arg0::startup_timing::mark(if enabled {
            "tui_startup_skills_coalescing_enabled"
        } else {
            "tui_startup_skills_coalescing_disabled"
        });
        if enabled {
            self.startup_skills_refresh_cwd = Some(self.config.cwd.to_path_buf());
        }
    }

    pub(super) fn normalize_session_skills_refresh(&mut self, event: AppEvent) -> Option<AppEvent> {
        let AppEvent::RefreshSessionSkills { cwd } = event else {
            return Some(event);
        };
        // Consume on the first session-origin event even if its cwd differs. A later session
        // must never inherit startup's exemption. Completion/failure of the original request
        // does not rearm it; SkillsListLoaded retains its normal result and stale-cwd handling.
        if self.startup_skills_refresh_cwd.take().as_deref() == Some(cwd.as_path())
            && self.config.cwd.as_path() == cwd.as_path()
        {
            codex_arg0::startup_timing::mark("tui_startup_skills_coalesced");
            return None;
        }
        Some(AppEvent::CodexOp(AppCommand::list_skills(
            vec![cwd],
            /*force_reload*/ true,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_policy_preserves_other_platforms_and_strict_override() {
        for (value, expected) in [(None, true), (Some("1"), true), (Some("0"), false)] {
            assert_eq!(
                startup_skills_coalesce_enabled(true, value.map(OsStr::new)),
                Ok(expected)
            );
        }
        for value in [None, Some("1"), Some("0"), Some("invalid")] {
            assert_eq!(
                startup_skills_coalesce_enabled(false, value.map(OsStr::new)),
                Ok(false)
            );
        }
        for value in ["", "true", "01", "1 ", "invalid"] {
            assert_eq!(
                startup_skills_coalesce_enabled(true, Some(OsStr::new(value))),
                Err(())
            );
        }
    }
}
