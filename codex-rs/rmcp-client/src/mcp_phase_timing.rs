//! Fixed-field TRUEOS diagnostics for MCP operation and HTTP progress.
//! No endpoint, credentials, request identifiers, or message content is retained.
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static NEXT_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
pub(crate) struct Capture(Arc<Context>);
struct Context {
    sequence: u64,
    started: Instant,
    layer: &'static str,
    method: &'static str,
    deadline_ms: AtomicU64,
    status_code: AtomicU64,
    span: tracing::Span,
    #[cfg(test)]
    records: std::sync::Mutex<Vec<Record>>,
}
#[cfg(test)]
#[derive(Clone, Debug)]
struct Record {
    phase: &'static str,
    state: &'static str,
    offset_ms: u128,
    duration_ms: u128,
    bytes: u64,
}

impl Capture {
    pub(crate) fn new(
        layer: &'static str,
        method: Option<&str>,
        deadline_ms: Option<u64>,
    ) -> Option<Self> {
        if !matches!(layer, "http" | "operation") {
            return None;
        }
        let method = match method? {
            "initialize" => "initialize",
            "notifications/initialized" => "notifications/initialized",
            "tools/list" => "tools/list",
            "server/discover" => "server/discover",
            _ => return None,
        };
        Some(Self(Arc::new(Context {
            sequence: NEXT_SEQUENCE.fetch_add(1, Ordering::Relaxed),
            started: Instant::now(),
            layer,
            method,
            deadline_ms: AtomicU64::new(deadline_ms.unwrap_or(u64::MAX)),
            status_code: AtomicU64::new(0),
            span: tracing::Span::current(),
            #[cfg(test)]
            records: Default::default(),
        })))
    }

    pub(crate) fn response_status(&self, status: u16) {
        self.0
            .status_code
            .store(u64::from(status), Ordering::Relaxed);
    }

    #[cfg(test)]
    pub(crate) fn test_records(&self) -> Vec<(&'static str, &'static str, u64)> {
        self.0
            .records
            .lock()
            .unwrap()
            .iter()
            .map(|r| (r.phase, r.state, r.bytes))
            .collect()
    }

    pub(crate) fn set_deadline(&self, deadline_ms: Option<u64>) {
        self.0
            .deadline_ms
            .store(deadline_ms.unwrap_or(u64::MAX), Ordering::Relaxed);
    }

    pub(crate) fn synchronous_phase(&self, phase: &'static str) -> Phase {
        let mut phase = self.phase(phase);
        // Synchronous preparation cannot be cancelled at an await boundary.
        phase.state = "error";
        phase
    }

    pub(crate) fn phase(&self, phase: &'static str) -> Phase {
        let started = Instant::now();
        self.record(phase, "started", started, Duration::ZERO, 0);
        Phase {
            capture: self.clone(),
            phase,
            started,
            finished: None,
            state: "cancelled",
            bytes: 0,
        }
    }

    fn record(
        &self,
        phase: &'static str,
        state: &'static str,
        started: Instant,
        duration: Duration,
        bytes: u64,
    ) {
        if !matches!(
            phase,
            "prepare"
                | "headers"
                | "body"
                | "first_body_byte"
                | "json_decode"
                | "first_sse_event"
                | "operation"
        ) {
            return;
        }
        let offset = started.saturating_duration_since(self.0.started);
        let deadline = self.0.deadline_ms.load(Ordering::Relaxed);
        let deadline_ms = (deadline != u64::MAX).then_some(deadline);
        let status_code = self.0.status_code.load(Ordering::Relaxed);
        #[cfg(any(target_os = "trueos", test))]
        tracing::info!(target: "codex_rmcp_client::phase", parent: &self.0.span, {
            event.name = "codex.mcp_phase",
            sequence = self.0.sequence, layer = self.0.layer, method = self.0.method,
            phase = phase, state = state, offset_ms = %offset.as_millis(), duration_ms = %duration.as_millis(),
            bytes = bytes, deadline_ms = deadline_ms, status_code = status_code,
        }, "MCP phase");
        #[cfg(test)]
        self.0.records.lock().unwrap().push(Record {
            phase,
            state,
            offset_ms: offset.as_millis(),
            duration_ms: duration.as_millis(),
            bytes,
        });
        #[cfg(not(any(target_os = "trueos", test)))]
        let _ = (
            phase,
            state,
            offset,
            duration,
            bytes,
            self.0.sequence,
            self.0.layer,
            self.0.method,
            deadline_ms,
            status_code,
            &self.0.span,
        );
    }
}

pub(crate) struct Phase {
    capture: Capture,
    phase: &'static str,
    started: Instant,
    finished: Option<Instant>,
    state: &'static str,
    bytes: u64,
}
impl Phase {
    pub(crate) fn finish(&mut self, error: bool) {
        if self.finished.is_none() {
            self.finished = Some(Instant::now());
            self.state = if error { "error" } else { "complete" };
        }
    }
    pub(crate) fn bytes(&mut self, count: usize) {
        if count != 0 && self.bytes == 0 {
            self.capture.record(
                "first_body_byte",
                "complete",
                self.started,
                self.started.elapsed(),
                count as u64,
            );
        }
        self.bytes = self.bytes.saturating_add(count as u64);
    }
}
impl Drop for Phase {
    fn drop(&mut self) {
        self.capture.record(
            self.phase,
            self.state,
            self.started,
            self.finished
                .unwrap_or_else(Instant::now)
                .saturating_duration_since(self.started),
            self.bytes,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dynamic_methods_are_not_recorded() {
        assert!(Capture::new("http", Some("tools/call/private-name"), None).is_none());
        assert!(Capture::new("http", None, None).is_none());
    }
    #[test]
    fn pending_phase_has_begin_and_one_cancelled_end() {
        let capture = Capture::new("http", Some("tools/list"), Some(30_000)).unwrap();
        let phase = capture.phase("headers");
        assert_eq!(capture.0.records.lock().unwrap().len(), 1);
        drop(phase);
        let records = capture.0.records.lock().unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(
            (records[0].state, records[1].state),
            ("started", "cancelled")
        );
        assert_eq!(records[0].offset_ms, records[1].offset_ms);
    }
    #[test]
    fn body_bytes_first_byte_and_completion_are_bounded() {
        let capture = Capture::new("http", Some("tools/list"), None).unwrap();
        let mut phase = capture.phase("body");
        phase.bytes(0);
        phase.bytes(3);
        phase.bytes(4);
        phase.finish(false);
        phase.finish(true);
        drop(phase);
        let records = capture.0.records.lock().unwrap();
        assert_eq!(records.len(), 3);
        assert_eq!(records[1].phase, "first_body_byte");
        assert_eq!((records[1].bytes, records[2].bytes), (3, 7));
        assert_eq!(records[2].state, "complete");
        assert!(records[1].duration_ms <= records[2].duration_ms);
    }
    #[test]
    fn returned_failure_is_distinct_from_cancellation() {
        let capture = Capture::new("operation", Some("tools/list"), Some(30_000)).unwrap();
        let mut phase = capture.phase("operation");
        phase.finish(true);
        drop(phase);
        assert_eq!(capture.0.records.lock().unwrap()[1].state, "error");
    }
}
