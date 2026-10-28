//! Opt-in carrier placement experiments, configured before thread admission.
//!
//! On TRUEOS, `$CODEX_HOME/trueos-threads.conf` (normally
//! `~/.codex/trueos-threads.conf`) can contain `single`, `isolated 2`, or
//! `spread 1`: a mode followed by an optional carrier cap from 1 to 4.
//! The UTF-8 file is limited to 128 bytes and two whitespace-separated tokens.
//! Either placement environment variable overrides the entire file; omitted
//! environment fields use their defaults. Missing profiles retain the baseline.

use std::io;
use std::io::Read;
use std::path::PathBuf;

const PLACEMENT_ENV: &str = "CODEX_TRUEOS_THREAD_PLACEMENT";
const BACKGROUND_CAP_ENV: &str = "CODEX_TRUEOS_BACKGROUND_CARRIERS";
const PROFILE_FILENAME: &str = "trueos-threads.conf";
const MAX_PROFILE_BYTES: usize = 128;

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

    fn parse_profile(bytes: &[u8]) -> io::Result<Self> {
        if bytes.len() > MAX_PROFILE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{PROFILE_FILENAME} exceeds {MAX_PROFILE_BYTES} bytes"),
            ));
        }
        let text = std::str::from_utf8(bytes).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{PROFILE_FILENAME} must be UTF-8"),
            )
        })?;
        let mut tokens = text.split_whitespace();
        let mode = tokens.next().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{PROFILE_FILENAME} must contain a mode"),
            )
        })?;
        let cap = tokens.next();
        if tokens.next().is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{PROFILE_FILENAME} accepts a mode and optional carrier cap"),
            ));
        }
        Self::parse(Some(mode), cap)
    }

    /// Return the selected policy and whether the user explicitly configured it.
    /// Resolve the profile path lazily so environment overrides perform no file IO.
    fn select(
        placement: Option<&str>,
        background_cap: Option<&str>,
        profile_path: impl FnOnce() -> io::Result<PathBuf>,
    ) -> io::Result<(Self, bool)> {
        if placement.is_some() || background_cap.is_some() {
            return Self::parse(placement, background_cap).map(|config| (config, true));
        }
        let path = profile_path()?;
        let file = match std::fs::File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Self::parse(None, None).map(|config| (config, false));
            }
            Err(error) => {
                return Err(io::Error::new(
                    error.kind(),
                    format!("cannot open {}: {error}", path.display()),
                ));
            }
        };
        let mut bytes = Vec::new();
        file.take((MAX_PROFILE_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!("cannot read {}: {error}", path.display()),
                )
            })?;
        Self::parse_profile(&bytes)
            .map(|config| (config, true))
            .map_err(|error| {
                io::Error::new(error.kind(), format!("invalid {}: {error}", path.display()))
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
    let (config, explicitly_configured) =
        PlacementConfig::select(placement.as_deref(), background_cap.as_deref(), || {
            Ok(super::find_codex_home()?.join(PROFILE_FILENAME).into())
        })?;
    let result = unsafe {
        trueos_cabi_thread_configure_placement_v1(config.placement as u32, config.background_cap)
    };
    // Helpers can run from a renamed archive or a kernel with this experiment
    // disabled. Keep ordinary startup when no experiment was requested;
    // explicit settings must still receive the platform error.
    if result == 95 && !explicitly_configured {
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

    struct TestHome(PathBuf);

    impl TestHome {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "codex-trueos-threads-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn profile(&self) -> PathBuf {
            self.0.join(PROFILE_FILENAME)
        }
    }

    impl Drop for TestHome {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

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

    #[test]
    fn profile_accepts_modes_optional_cap_and_rejects_bad_boundaries() {
        assert_eq!(
            PlacementConfig::parse_profile(b"single\n")
                .unwrap()
                .placement,
            Placement::Single
        );
        assert_eq!(
            PlacementConfig::parse_profile(b"isolated 2\n").unwrap(),
            PlacementConfig {
                placement: Placement::Isolated,
                background_cap: 2
            }
        );
        assert_eq!(
            PlacementConfig::parse_profile(b"spread 1").unwrap(),
            PlacementConfig {
                placement: Placement::Spread,
                background_cap: 1
            }
        );
        for bytes in [
            b"".as_slice(),
            b" \n",
            b"single 2 third",
            b"parallel",
            b"spread 0",
            b"spread 5",
            b"\xff",
        ] {
            assert!(
                PlacementConfig::parse_profile(bytes).is_err(),
                "accepted {bytes:?}"
            );
        }
        let mut bounded = b"single".to_vec();
        bounded.resize(MAX_PROFILE_BYTES, b' ');
        assert!(PlacementConfig::parse_profile(&bounded).is_ok());
        bounded.push(b' ');
        assert_eq!(
            PlacementConfig::parse_profile(&bounded).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn temporary_profile_selects_policy_and_missing_file_uses_baseline() {
        let home = TestHome::new();
        assert_eq!(
            PlacementConfig::select(None, None, || Ok(home.profile())).unwrap(),
            (PlacementConfig::parse(None, None).unwrap(), false)
        );
        std::fs::write(home.profile(), b"isolated 3\n").unwrap();
        assert_eq!(
            PlacementConfig::select(None, None, || Ok(home.profile())).unwrap(),
            (
                PlacementConfig {
                    placement: Placement::Isolated,
                    background_cap: 3
                },
                true
            )
        );
        std::fs::write(home.profile(), vec![b'x'; MAX_PROFILE_BYTES + 1]).unwrap();
        assert_eq!(
            PlacementConfig::select(None, None, || Ok(home.profile()))
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        std::fs::remove_file(home.profile()).unwrap();
        std::fs::create_dir(home.profile()).unwrap();
        assert!(
            PlacementConfig::select(None, None, || Ok(home.profile())).is_err(),
            "profile read error became a missing-file fallback"
        );
    }

    #[test]
    fn either_environment_setting_skips_profile_and_resets_omitted_fields() {
        let fail_if_read =
            || -> io::Result<PathBuf> { panic!("environment override resolved profile") };
        assert_eq!(
            PlacementConfig::select(Some("spread"), None, fail_if_read).unwrap(),
            (
                PlacementConfig {
                    placement: Placement::Spread,
                    background_cap: 2
                },
                true
            )
        );
        assert_eq!(
            PlacementConfig::select(None, Some("4"), fail_if_read).unwrap(),
            (
                PlacementConfig {
                    placement: Placement::Single,
                    background_cap: 4
                },
                true
            )
        );
        assert!(PlacementConfig::select(Some("invalid"), None, fail_if_read).is_err());
    }
}
