//! Opt-in, process-local startup milestones for terminal bring-up measurements.
//! Set CODEX_STARTUP_TIMING_FILE for TSV rows, or CODEX_STARTUP_TIMING_STDERR=1
//! for visible diagnostics during terminal bring-up.

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

/// Record a milestone once per process, with elapsed and previous-milestone ms.
/// Disabled unless a sink is explicitly requested; never blocks startup on an error.
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
    let row = format!(
        "{}\t{:.3}\t{:.3}\t{}\n",
        std::process::id(),
        now.duration_since(timing.start).as_secs_f64() * 1000.0,
        now.duration_since(timing.previous).as_secs_f64() * 1000.0,
        stage,
    );
    timing.previous = now;
    if let Some(file) = timing.file.as_mut() {
        let _ = file.write_all(row.as_bytes());
    }
    if timing.stderr {
        let _ = std::io::stderr().write_all(format!("[codex-startup] {row}").as_bytes());
    }
}
