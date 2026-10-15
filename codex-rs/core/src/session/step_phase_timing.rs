//! Numeric-only timing for overlapping step captures on TRUEOS.
use codex_otel::SessionTelemetry;
use std::future::Future;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Instant;
use tokio_util::sync::CancellationToken;

static NEXT_CAPTURE: AtomicU64 = AtomicU64::new(1);

pub(super) struct Capture {
    id: u64,
    started: Instant,
    telemetry: SessionTelemetry,
    cancellation: CancellationToken,
    #[cfg(test)]
    records: std::sync::Mutex<Vec<TestRecord>>,
}

impl Capture {
    pub(super) fn new(telemetry: &SessionTelemetry, cancellation: &CancellationToken) -> Self {
        Self {
            id: NEXT_CAPTURE.fetch_add(1, Ordering::Relaxed),
            started: Instant::now(),
            telemetry: telemetry.clone(),
            cancellation: cancellation.clone(),
            #[cfg(test)]
            records: std::sync::Mutex::new(Vec::new()),
        }
    }

    pub(super) fn phase(&self, phase: &'static str) -> Phase<'_> {
        Phase {
            capture: self,
            phase,
            started: Instant::now(),
            status: "cancelled",
        }
    }

    /// Publish before calling work that may never return or yield its poll.
    pub(super) fn started_phase(&self, phase: &'static str) -> Phase<'_> {
        let timing = self.phase(phase);
        timing.record("started", std::time::Duration::ZERO);
        timing
    }

    pub(super) async fn run_started<T>(
        &self,
        phase: &'static str,
        future: impl Future<Output = T>,
    ) -> T {
        let mut timing = self.started_phase(phase);
        let value = future.await;
        timing.finish(false);
        value
    }

    pub(super) async fn result_started<T, E>(
        &self,
        phase: &'static str,
        future: impl Future<Output = Result<T, E>>,
    ) -> Result<T, E> {
        let mut timing = self.started_phase(phase);
        let value = future.await;
        timing.finish(value.is_err());
        value
    }

    pub(super) async fn run<T>(&self, phase: &'static str, future: impl Future<Output = T>) -> T {
        let mut timing = self.phase(phase);
        let value = future.await;
        timing.finish(false);
        value
    }

    pub(super) async fn result<T, E>(
        &self,
        phase: &'static str,
        future: impl Future<Output = Result<T, E>>,
    ) -> Result<T, E> {
        let mut timing = self.phase(phase);
        let value = future.await;
        timing.finish(value.is_err());
        value
    }
}

pub(super) struct Phase<'a> {
    capture: &'a Capture,
    phase: &'static str,
    started: Instant,
    // Dropping an unfinished future is cancellation, never successful completion.
    status: &'static str,
}

impl Phase<'_> {
    pub(super) fn finish(&mut self, failed: bool) {
        self.status = if self.capture.cancellation.is_cancelled() {
            "cancelled"
        } else if failed {
            "error"
        } else {
            "complete"
        };
    }

    fn record(&self, status: &'static str, duration: std::time::Duration) {
        let offset = self.started.saturating_duration_since(self.capture.started);
        #[cfg(test)]
        self.capture.records.lock().unwrap().push(TestRecord {
            capture_id: self.capture.id,
            phase: self.phase,
            offset_ms: offset.as_millis(),
            duration_ms: duration.as_millis(),
            status,
        });
        self.capture.telemetry.record_step_context_phase(
            self.capture.id,
            self.phase,
            offset,
            duration,
            status,
        );
    }
}

impl Drop for Phase<'_> {
    fn drop(&mut self) {
        self.record(self.status, self.started.elapsed());
    }
}

#[cfg(test)]
struct TestRecord {
    capture_id: u64,
    phase: &'static str,
    offset_ms: u128,
    duration_ms: u128,
    status: &'static str,
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_protocol::ThreadId;
    use codex_protocol::protocol::SessionSource;
    use std::task::Context;
    use std::task::Poll;
    use std::task::Waker;

    fn capture(token: &CancellationToken) -> Capture {
        let telemetry = SessionTelemetry::new(
            ThreadId::new(),
            "test",
            "test",
            None,
            None,
            None,
            "test".to_string(),
            false,
            "test".to_string(),
            SessionSource::Cli,
        );
        Capture::new(&telemetry, token)
    }

    #[test]
    fn completed_results_record_one_status_and_share_capture_clock() {
        let capture = capture(&CancellationToken::new());
        let origin = capture.started;
        let mut context = Context::from_waker(Waker::noop());
        let mut success =
            Box::pin(capture.result("build_router", std::future::ready(Ok::<_, ()>(7))));
        assert_eq!(success.as_mut().poll(&mut context), Poll::Ready(Ok(7)));
        drop(success);
        let mut failure =
            Box::pin(capture.result("instructions", std::future::ready(Err::<(), _>("failed"))));
        assert_eq!(
            failure.as_mut().poll(&mut context),
            Poll::Ready(Err("failed"))
        );
        drop(failure);
        let records = capture.records.lock().unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].status, "complete");
        assert_eq!(records[1].status, "error");
        assert_eq!(records[0].phase, "build_router");
        assert_eq!(records[1].phase, "instructions");
        for record in records.iter() {
            assert_eq!(record.capture_id, capture.id);
            assert!(record.offset_ms + record.duration_ms <= origin.elapsed().as_millis());
        }
        assert!(records[0].offset_ms <= records[1].offset_ms);
    }

    #[test]
    fn dropping_polled_future_records_cancellation_once() {
        let capture = capture(&CancellationToken::new());
        let mut context = Context::from_waker(Waker::noop());
        let mut future = Box::pin(capture.run("mcp_ready", std::future::pending::<()>()));
        assert!(future.as_mut().poll(&mut context).is_pending());
        assert!(capture.records.lock().unwrap().is_empty());
        drop(future);
        let records = capture.records.lock().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].status, "cancelled");
    }

    #[test]
    fn token_cancellation_is_not_reported_as_success_or_error() {
        let token = CancellationToken::new();
        let capture = capture(&token);
        token.cancel();
        let mut context = Context::from_waker(Waker::noop());
        let mut future = Box::pin(capture.result(
            "environment_ready",
            std::future::ready(Err::<(), _>("cancelled")),
        ));
        assert_eq!(
            future.as_mut().poll(&mut context),
            Poll::Ready(Err("cancelled"))
        );
        drop(future);
        let records = capture.records.lock().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].status, "cancelled");
        assert_ne!(
            capture.id,
            super::Capture::new(&capture.telemetry, &token).id
        );
    }

    #[test]
    fn started_boundary_precedes_first_poll_and_survives_pending_cancellation() {
        let capture = capture(&CancellationToken::new());
        let mut context = Context::from_waker(Waker::noop());
        let mut future = Box::pin(capture.run_started("turn_display_roots", async {
            // This assertion runs inside the first poll of the measured work.
            assert_eq!(capture.records.lock().unwrap()[0].status, "started");
            std::future::pending::<()>().await;
        }));
        assert!(capture.records.lock().unwrap().is_empty());
        assert!(future.as_mut().poll(&mut context).is_pending());
        assert_eq!(capture.records.lock().unwrap().len(), 1);
        drop(future);
        let records = capture.records.lock().unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].status, "started");
        assert_eq!(records[0].duration_ms, 0);
        assert_eq!(records[1].status, "cancelled");
        assert_eq!(records[0].capture_id, records[1].capture_id);
        assert_eq!(records[0].offset_ms, records[1].offset_ms);
    }

    #[test]
    fn synchronous_stream_entry_and_failed_await_have_started_and_terminal_boundaries() {
        let capture = capture(&CancellationToken::new());
        {
            let mut phase = capture.started_phase("sampling_build_prompt");
            assert_eq!(
                capture.records.lock().unwrap().last().unwrap().status,
                "started"
            );
            phase.finish(false);
        }
        let mut context = Context::from_waker(Waker::noop());
        let mut future = Box::pin(capture.result_started(
            "sampling_stream",
            std::future::ready(Err::<(), _>("private error")),
        ));
        assert_eq!(
            future.as_mut().poll(&mut context),
            Poll::Ready(Err("private error"))
        );
        drop(future);
        let records = capture.records.lock().unwrap();
        assert_eq!(
            records
                .iter()
                .map(|record| record.status)
                .collect::<Vec<_>>(),
            ["started", "complete", "started", "error"]
        );
    }
}
