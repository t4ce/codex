//! Opt-in carrier placement experiments, configured before thread admission.

use std::io;

const PLACEMENT_ENV: &str = "CODEX_TRUEOS_THREAD_PLACEMENT";
const BACKGROUND_CAP_ENV: &str = "CODEX_TRUEOS_BACKGROUND_CARRIERS";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
enum Placement {
    Single = 0,
    Isolated = 1,
    Spread = 2,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PlacementConfig {
    placement: Placement,
    background_cap: u32,
}

impl PlacementConfig {
    fn parse(placement: Option<&str>, background_cap: Option<&str>) -> io::Result<Self> {
        let placement = match placement {
            None | Some("single") => Placement::Single,
            Some("isolated") => Placement::Isolated,
            Some("spread") => Placement::Spread,
            Some(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{PLACEMENT_ENV} must be single, isolated, or spread"),
                ));
            }
        };
        let background_cap = match background_cap {
            None => 2,
            Some(value) => value
                .parse::<u32>()
                .ok()
                .filter(|cap| (1..=4).contains(cap))
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("{BACKGROUND_CAP_ENV} must be an integer from 1 to 4"),
                    )
                })?,
        };
        Ok(Self {
            placement,
            background_cap,
        })
    }
}

#[cfg(target_os = "trueos")]
unsafe extern "C" {
    fn trueos_cabi_thread_configure_placement_v1(mode: u32, background_cap: u32) -> i32;
    fn trueos_cabi_thread_swap_child_role_v1(role: u32) -> u32;
    fn trueos_cabi_thread_swap_activity_v1(activity: u32) -> u32;
}

#[cfg(target_os = "trueos")]
pub(crate) fn configure_from_env() -> io::Result<()> {
    fn setting(name: &str) -> io::Result<Option<String>> {
        match std::env::var(name) {
            Ok(value) => Ok(Some(value)),
            Err(std::env::VarError::NotPresent) => Ok(None),
            Err(std::env::VarError::NotUnicode(_)) => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{name} must contain a UTF-8 value"),
            )),
        }
    }

    let placement = setting(PLACEMENT_ENV)?;
    let background_cap = setting(BACKGROUND_CAP_ENV)?;
    let config = PlacementConfig::parse(placement.as_deref(), background_cap.as_deref())?;
    let result = unsafe {
        trueos_cabi_thread_configure_placement_v1(config.placement as u32, config.background_cap)
    };
    // Helpers can run from a renamed archive or a kernel with this experiment
    // disabled. Keep ordinary startup when no experiment was requested;
    // explicit settings must still receive the platform error.
    if result == 95 && placement.is_none() && background_cap.is_none() {
        tracing::info!(target: "codex_startup", "default thread placement unavailable");
        crate::startup_timing::mark("thread_placement_unavailable_default");
        return Ok(());
    }
    if result != 0 {
        return Err(io::Error::other(format!(
            "TRUEOS thread placement configuration failed: {}",
            io::Error::from_raw_os_error(result)
        )));
    }
    tracing::info!(
        target: "codex_startup",
        placement = ?config.placement,
        background_cap = config.background_cap,
        "thread placement configured"
    );
    crate::startup_timing::mark(&format!(
        "thread_placement_configured mode={} background_cap={}",
        config.placement as u32, config.background_cap
    ));
    Ok(())
}

#[cfg(target_os = "trueos")]
pub(crate) struct ChildRoleGuard(u32, std::marker::PhantomData<*mut ()>);

#[cfg(target_os = "trueos")]
impl ChildRoleGuard {
    pub(crate) fn foreground() -> Self {
        Self(
            unsafe { trueos_cabi_thread_swap_child_role_v1(1) },
            std::marker::PhantomData,
        )
    }
}

#[cfg(target_os = "trueos")]
impl Drop for ChildRoleGuard {
    fn drop(&mut self) {
        if self.0 != u32::MAX {
            unsafe { trueos_cabi_thread_swap_child_role_v1(self.0) };
        }
    }
}

#[cfg(target_os = "trueos")]
pub(crate) struct ActivityGuard(u32, std::marker::PhantomData<*mut ()>);

#[cfg(target_os = "trueos")]
impl ActivityGuard {
    pub(crate) fn foreground() -> Self {
        Self(
            unsafe { trueos_cabi_thread_swap_activity_v1(1) },
            std::marker::PhantomData,
        )
    }
}

#[cfg(target_os = "trueos")]
impl Drop for ActivityGuard {
    fn drop(&mut self) {
        if self.0 != u32::MAX {
            unsafe { trueos_cabi_thread_swap_activity_v1(self.0) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_retains_single_carrier() {
        assert_eq!(
            PlacementConfig::parse(None, None).unwrap(),
            PlacementConfig {
                placement: Placement::Single,
                background_cap: 2,
            }
        );
    }

    #[test]
    fn accepts_each_mode_and_bounded_cap() {
        for (value, expected) in [
            ("single", Placement::Single),
            ("isolated", Placement::Isolated),
            ("spread", Placement::Spread),
        ] {
            for cap in 1..=4 {
                let parsed = PlacementConfig::parse(Some(value), Some(&cap.to_string())).unwrap();
                assert_eq!(parsed.placement, expected);
                assert_eq!(parsed.background_cap, cap);
            }
        }
    }

    #[test]
    fn rejects_invalid_values_without_changing_environment() {
        for value in ["", "parallel", "ISOLATED", " single"] {
            let error = PlacementConfig::parse(Some(value), None).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
            assert!(error.to_string().contains(PLACEMENT_ENV));
        }
        for value in ["", "0", "5", "-1", "many", "1.5", "4294967296"] {
            let error = PlacementConfig::parse(Some("spread"), Some(value)).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
            assert!(error.to_string().contains(BACKGROUND_CAP_ENV));
        }
    }
}
