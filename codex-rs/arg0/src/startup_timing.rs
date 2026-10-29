//! Opt-in, process-local startup milestones for terminal bring-up measurements.
//! Set CODEX_STARTUP_TIMING_FILE for TSV rows, or CODEX_STARTUP_TIMING_STDERR=1
//! for diagnostics during terminal bring-up. On TRUEOS, diagnostics go to the
//! resident logger directly, including before tracing has a subscriber, so
//! milestones emitted after drawing cannot overwrite the TUI.

use std::collections::BTreeSet;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Instant;

struct Timing {
    file: Option<File>,
    stderr: bool,
    start: Instant,
    previous: Instant,
    seen: BTreeSet<String>,
}

static TIMING: OnceLock<Option<Mutex<Timing>>> = OnceLock::new();

#[cfg(target_os = "trueos")]
unsafe extern "C" {
    fn trueos_cabi_log(
        level: u32,
        target_ptr: *const u8,
        target_len: usize,
        message_ptr: *const u8,
        message_len: usize,
    ) -> i32;
}

/// Record a milestone once per process, with elapsed and previous-milestone ms.
/// Disabled unless a sink is explicitly requested; sink errors are ignored.
pub fn mark(stage: &str) {
    let timing = TIMING.get_or_init(|| {
        let path = std::env::var_os("CODEX_STARTUP_TIMING_FILE");
        let stderr = std::env::var("CODEX_STARTUP_TIMING_STDERR").is_ok_and(|value| value == "1");
        if path.is_none() && !stderr {
            return None;
        }
        let start = Instant::now();
        let file =
            path.and_then(|path| OpenOptions::new().create(true).append(true).open(path).ok());
        Some(Mutex::new(Timing {
            file,
            stderr,
            start,
            previous: start,
            seen: BTreeSet::new(),
        }))
    });
    let Some(timing) = timing else { return };
    let Ok(mut timing) = timing.lock() else {
        return;
    };
    if !timing.seen.insert(stage.to_owned()) {
        return;
    }
    let now = Instant::now();
    let pid = std::process::id();
    let elapsed_ms = now.duration_since(timing.start).as_secs_f64() * 1000.0;
    let delta_ms = now.duration_since(timing.previous).as_secs_f64() * 1000.0;
    let row = format!("{pid}\t{elapsed_ms:.3}\t{delta_ms:.3}\t{stage}\n");
    timing.previous = now;
    if let Some(file) = timing.file.as_mut() {
        let _ = file.write_all(row.as_bytes());
    }
    if timing.stderr {
        #[cfg(target_os = "trueos")]
        {
            // Placement and arg0 milestones precede tracing initialization.
            // The resident CABI logger is available from entry and keeps these
            // records out of the terminal byte stream throughout TUI startup.
            const INFO: u32 = 3; // trueos_v::vsys::LOG_LEVEL_INFO
            const TARGET: &[u8] = b"codex_startup";
            let message = format!("[codex-startup] {}", row.trim_end());
            let _ = unsafe {
                trueos_cabi_log(
                    INFO,
                    TARGET.as_ptr(),
                    TARGET.len(),
                    message.as_ptr(),
                    message.len(),
                )
            };
        }
        #[cfg(not(target_os = "trueos"))]
        let _ = std::io::stderr()
            .write_all(format!("[codex-startup] {}\r\n", row.trim_end()).as_bytes());
    }
}
