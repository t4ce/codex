//! Opt-in startup measurements shared before any tracing subscriber exists.
//! Set CODEX_STARTUP_TIMING_FILE for TSV rows, or CODEX_STARTUP_TIMING_STDERR=1
//! for diagnostics during terminal bring-up. On TRUEOS, diagnostics go to the
//! resident logger directly, including before tracing has a subscriber, so
//! milestones emitted after drawing cannot overwrite the TUI.

use std::collections::BTreeSet;
use std::fs::File;
use std::fs::OpenOptions;
use std::future::Future;
use std::future::poll_fn;
use std::io::Write;
use std::pin::pin;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

struct Timing {
    file: Option<File>,
    stderr: bool,
    start: Instant,
    previous: Instant,
    seen: BTreeSet<String>,
    context: Option<String>,
    request: Option<u64>,
}

static TIMING: OnceLock<Option<Mutex<Timing>>> = OnceLock::new();
static NEXT_SPAN: AtomicU64 = AtomicU64::new(1);
// These probes target startup, rather than collecting an unbounded session trace.
const MAX_SPANS: u64 = 256;

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

fn timing() -> Option<&'static Mutex<Timing>> {
    TIMING
        .get_or_init(|| {
            let path = std::env::var_os("CODEX_STARTUP_TIMING_FILE");
            let stderr =
                std::env::var("CODEX_STARTUP_TIMING_STDERR").is_ok_and(|value| value == "1");
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
                context: process_context(),
                request: std::env::var("TRUEOS_STARTUP_REQUEST")
                    .ok()
                    .and_then(|value| {
                        value
                            .bytes()
                            .all(|byte| byte.is_ascii_digit())
                            .then(|| value.parse().ok())
                            .flatten()
                    }),
            }))
        })
        .as_ref()
}

/// Record a milestone once per process. Sink errors are ignored.
pub fn mark(stage: &str) {
    emit(stage, true);
}

fn emit(stage: &str, once: bool) {
    let Some(timing) = timing() else { return };
    let Ok(mut timing) = timing.lock() else {
        return;
    };
    if once && !timing.seen.insert(stage.to_owned()) {
        return;
    }
    if let Some(context) = timing.context.take() {
        write_row(&mut timing, &context);
    }
    write_row(&mut timing, stage);
}

fn process_context() -> Option<String> {
    let mut fields = Vec::new();
    for (variable, key) in [
        ("TRUEOS_STARTUP_REQUEST", "request"),
        ("TRUEOS_APP_GENERATION", "app_gen"),
    ] {
        if let Ok(value) = std::env::var(variable)
            && !value.is_empty()
            && value.bytes().all(|byte| byte.is_ascii_digit())
            && let Ok(number) = value.parse::<u64>()
        {
            fields.push(format!("{key}={number}"));
        }
    }
    if let Ok(value) = std::env::var("TRUEOS_APP_INSTANCE")
        && value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if [8, 13, 18, 23].contains(&index) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
    {
        fields.push(format!("instance={value}"));
    }
    if let Ok(value) = std::env::var("TRUEOS_APP_SHA256")
        && value.len() == 64
        && value.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        fields.push(format!("archive_sha256={}", value.to_ascii_lowercase()));
    }
    (!fields.is_empty()).then(|| format!("process_context {}", fields.join(" ")))
}

fn write_row(timing: &mut Timing, stage: &str) {
    let now = Instant::now();
    let pid = std::process::id();
    let elapsed_ms = now.duration_since(timing.start).as_secs_f64() * 1000.0;
    let delta_ms = now.duration_since(timing.previous).as_secs_f64() * 1000.0;
    // Native TRUEOS processes can all report pid=1. Carry the launch identity
    // on every record so simultaneous applications cannot be conflated.
    let request = timing
        .request
        .map(|id| format!(" request={id}"))
        .unwrap_or_default();
    let row = format!("{pid}\t{elapsed_ms:.3}\t{delta_ms:.3}\t{stage}{request}\n");
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

/// One bounded phase measurement. Dropping an unfinished phase records cancellation.
/// Times are wall elapsed, including descheduling and synchronous host calls, not CPU time.
pub struct Span {
    active: Option<ActiveSpan>,
}

struct ActiveSpan {
    id: u64,
    name: &'static str,
    start: Instant,
    polls: u64,
    poll_total: Duration,
    poll_max: Duration,
}

fn label_is_valid(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= 96
        && label
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}

/// Start a repeatable named phase; labels must be static identifiers, never paths or user data.
pub fn span(name: &'static str) -> Span {
    if timing().is_none() || !label_is_valid(name) {
        return Span { active: None };
    }
    let id = NEXT_SPAN.fetch_add(1, Ordering::Relaxed);
    if id > MAX_SPANS {
        mark("timing_span_budget_exhausted");
        return Span { active: None };
    }
    let active = ActiveSpan {
        id,
        name,
        start: Instant::now(),
        polls: 0,
        poll_total: Duration::ZERO,
        poll_max: Duration::ZERO,
    };
    emit(&format!("span_begin id={} name={name}", active.id), false);
    Span {
        active: Some(active),
    }
}

impl Span {
    /// End a phase with a static result such as `ok`, `error`, `busy`, or `complete`.
    pub fn finish(mut self, outcome: &'static str) {
        self.end(if label_is_valid(outcome) {
            outcome
        } else {
            "invalid_outcome"
        });
    }

    fn end(&mut self, outcome: &'static str) {
        let Some(active) = self.active.take() else {
            return;
        };
        emit(
            &format!(
                "span_end id={} name={} outcome={} duration_ms={:.3} poll_n={} poll_total_ms={:.3} poll_max_ms={:.3}",
                active.id,
                active.name,
                outcome,
                active.start.elapsed().as_secs_f64() * 1000.0,
                active.polls,
                active.poll_total.as_secs_f64() * 1000.0,
                active.poll_max.as_secs_f64() * 1000.0,
            ),
            false,
        );
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        self.end("cancelled");
    }
}

/// Measure an existing future without spawning it or changing its executor/input ownership.
/// A large maximum poll exposes work that can hold a cooperative input loop hostage.
/// Pending time is separate from time inside polls; neither is a CPU measurement.
pub async fn measure<F: Future>(name: &'static str, future: F) -> F::Output {
    let mut phase = span(name);
    if phase.active.is_none() {
        return future.await;
    }
    let mut future = pin!(future);
    let result = poll_fn(|context| {
        let start = Instant::now();
        let output = future.as_mut().poll(context);
        if let Some(active) = phase.active.as_mut() {
            let elapsed = start.elapsed();
            active.polls = active.polls.saturating_add(1);
            active.poll_total = active.poll_total.saturating_add(elapsed);
            active.poll_max = active.poll_max.max(elapsed);
        }
        output
    })
    .await;
    phase.finish("complete");
    result
}
