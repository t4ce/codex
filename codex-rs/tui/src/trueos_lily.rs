//! Text-only response tee to the independent Lily Blueprint VM.
//!
//! The TUI never performs inference or waits for HTTP/readiness. Its watch slot
//! retains only the newest bounded snapshot; the background worker owns launch,
//! readiness and transport. The service owns VNNI and all Spirit signaling.
use serde::Deserialize;
use serde::Serialize;
use std::time::Duration;
use std::time::Instant;
use tokio::sync::watch;

const ENDPOINT: &str = "http://127.0.0.1:9771";
#[cfg(target_os = "trueos")]
const START_SCRIPT: &str = "launch --headless lily2vec\n";
const SOFT_CHARS: usize = 384;
const RETAIN_CHARS: usize = 768;
const MIN_SENTENCE_CHARS: usize = 96;
const INTERVAL: Duration = Duration::from_millis(2500);
const MAX_PENDING_AGE: Duration = Duration::from_secs(10);
const STARTUP_TIMEOUT: Duration = Duration::from_secs(20);
const RETRY_DELAY: Duration = Duration::from_secs(5);

fn snapshot(text: &str) -> String {
    let mut chars: Vec<_> = text.char_indices().rev().take(RETAIN_CHARS + 1).collect();
    chars.reverse();
    if chars.len() <= SOFT_CHARS {
        return text.to_owned();
    }
    let hard = chars.len().saturating_sub(RETAIN_CHARS);
    let soft = chars.len() - SOFT_CHARS;
    let natural = (hard..=soft).rev().find(|&i| {
        chars[i].0 == 0
            || (i > 0
                && (chars[i - 1].1 == '\n'
                    || (chars[i].1.is_whitespace() && matches!(chars[i - 1].1, '.' | '!' | '?'))))
    });
    let word = (hard..=soft)
        .rev()
        .find(|&i| chars[i].0 == 0 || (i > 0 && chars[i - 1].1.is_whitespace()));
    text[chars[natural.or(word).unwrap_or(hard)].0..].to_owned()
}

#[derive(Default)]
struct Sampler {
    pending: String,
    last_sample: Option<Duration>,
}
impl Sampler {
    fn due(&self, now: Duration) -> bool {
        self.last_sample
            .is_none_or(|last| now.saturating_sub(last) >= INTERVAL)
    }
    fn push(&mut self, delta: &str, now: Duration) -> Option<String> {
        let tail = snapshot(delta);
        if tail.len() != delta.len() {
            self.pending = tail;
        } else {
            self.pending.push_str(&tail);
        }
        if self.pending.chars().count() > RETAIN_CHARS {
            self.pending = snapshot(&self.pending);
        }
        let count = self.pending.chars().count();
        let sentence_end = self.pending.trim_end().ends_with(['.', '!', '?']);
        if self.due(now)
            && (count >= SOFT_CHARS
                || self.pending.contains("\n\n")
                || (count >= MIN_SENTENCE_CHARS && sentence_end))
        {
            self.last_sample = Some(now);
            return Some(snapshot(&std::mem::take(&mut self.pending)));
        }
        None
    }
    fn finish(&mut self, response: Option<&str>, now: Duration) -> Option<String> {
        let text = response
            .map(snapshot)
            .unwrap_or_else(|| std::mem::take(&mut self.pending));
        self.pending.clear();
        if text.trim().is_empty() || !self.due(now) {
            return None;
        }
        self.last_sample = Some(now);
        Some(snapshot(&text))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Snapshot {
    source: String,
    sequence: u64,
    text: String,
    clear: bool,
    // Monotonic local age is transport bookkeeping, not a service protocol field.
    #[serde(skip, default = "Instant::now")]
    queued_at: Instant,
}
impl Snapshot {
    fn expired(&self, now: Instant) -> bool {
        !self.clear && now.saturating_duration_since(self.queued_at) >= MAX_PENDING_AGE
    }
}

pub(crate) struct ResponsePresentation {
    started: Instant,
    sampler: Sampler,
    source: String,
    sequence: u64,
    latest: watch::Sender<Option<Snapshot>>,
}
impl Default for ResponsePresentation {
    fn default() -> Self {
        let source = format!("codex-{}", uuid::Uuid::new_v4());
        let (latest, receiver) = watch::channel(None);
        // Start readiness/launch immediately, independently of the first reply.
        tokio::spawn(worker(receiver, ENDPOINT.to_owned()));
        Self {
            started: Instant::now(),
            sampler: Sampler::default(),
            source,
            sequence: 0,
            latest,
        }
    }
}
impl ResponsePresentation {
    fn send(&mut self, text: String, clear: bool) {
        self.sequence = self.sequence.saturating_add(1);
        self.latest.send_replace(Some(Snapshot {
            source: self.source.clone(),
            sequence: self.sequence,
            text,
            clear,
            queued_at: Instant::now(),
        }));
    }
    pub(crate) fn delta(&mut self, text: &str) {
        if let Some(text) = self.sampler.push(text, self.started.elapsed()) {
            self.send(text, false);
        }
    }
    pub(crate) fn finish(&mut self, response: Option<&str>) {
        if let Some(text) = self.sampler.finish(response, self.started.elapsed()) {
            self.send(text, false);
        }
    }
    pub(crate) fn clear(&mut self) {
        self.sampler.pending.clear();
        self.send(String::new(), true);
    }
}
impl Drop for ResponsePresentation {
    fn drop(&mut self) {
        // The worker flushes this final clear with its bounded request timeout,
        // then exits when the watch channel closes. The service outlives Codex.
        self.clear();
    }
}

#[derive(Deserialize)]
struct Health {
    protocol: String,
    backend: String,
    ready: bool,
}

fn client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_millis(250))
        .timeout(Duration::from_millis(750))
        .pool_max_idle_per_host(1)
        .build()
}
async fn health_ready(client: &reqwest::Client, endpoint: &str) -> bool {
    let Ok(response) = client.get(format!("{endpoint}/health")).send().await else {
        return false;
    };
    if !response.status().is_success() {
        return false;
    }
    let Ok(health) = response.json::<Health>().await else {
        return false;
    };
    health.ready && health.protocol == "lily2vec-v1" && health.backend == "avx-vnni"
}

fn launch_service() -> Result<(), String> {
    #[cfg(target_os = "trueos")]
    return trueos::vshell::launch_with_destination(
        "apps",
        START_SCRIPT,
        trueos::vshell::LaunchDestination::Headless,
    )
    .map_err(|code| format!("Lily Blueprint launch failed ({code})"));
    #[cfg(not(target_os = "trueos"))]
    Err("Lily Blueprint launch requires TRUEOS".into())
}

async fn ensure_service(client: &reqwest::Client, endpoint: &str) -> Result<(), String> {
    // Same launch ownership as sqlx-sqlite's native ensure_service: reuse a
    // healthy localhost service, otherwise request Apps/headless and wait.
    static LAUNCH: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);
    let _permit = LAUNCH.acquire().await.map_err(|error| error.to_string())?;
    if health_ready(client, endpoint).await {
        return Ok(());
    }
    launch_service()?;
    let deadline = tokio::time::Instant::now() + STARTUP_TIMEOUT;
    loop {
        if health_ready(client, endpoint).await {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("Lily service readiness timed out".into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn post(client: &reqwest::Client, endpoint: &str, snapshot: &Snapshot) -> Result<(), String> {
    let response = client
        .post(format!("{endpoint}/v1/snapshots"))
        .json(snapshot)
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if response.status() == reqwest::StatusCode::ACCEPTED {
        Ok(())
    } else {
        Err(format!("Lily snapshot rejected ({})", response.status()))
    }
}

async fn final_clear(
    client: &reqwest::Client,
    endpoint: &str,
    receiver: &watch::Receiver<Option<Snapshot>>,
) {
    let message = receiver.borrow().clone().filter(|s| s.clear);
    if let Some(message) = message {
        let _ = post(client, endpoint, &message).await;
    }
}

async fn worker(mut receiver: watch::Receiver<Option<Snapshot>>, endpoint: String) {
    let client = match client() {
        Ok(client) => client,
        Err(error) => {
            tracing::warn!(%error, "Lily response transport unavailable");
            return;
        }
    };
    let mut ready = false;
    let mut current = None;
    let mut last_error = None;
    loop {
        if !ready {
            if receiver.has_changed().is_err() {
                // A prior POST may have been admitted even if its response
                // failed. Do not lose the final cancellation while reconnecting.
                final_clear(&client, &endpoint, &receiver).await;
                return;
            }
            match ensure_service(&client, &endpoint).await {
                Ok(()) => {
                    ready = true;
                    last_error = None;
                }
                Err(error) => {
                    if last_error.as_ref() != Some(&error) {
                        tracing::warn!(%error,"Lily service unavailable; response tee retained latest snapshot");
                    }
                    last_error = Some(error);
                    tokio::time::sleep(RETRY_DELAY).await;
                    continue;
                }
            }
            // Startup may have taken time; send only the latest response.
            current = receiver.borrow_and_update().clone();
        }
        if let Some(snapshot) = current
            .take()
            .filter(|s: &Snapshot| !s.expired(Instant::now()))
        {
            tokio::select! {
                changed = receiver.changed() => {
                    if changed.is_err() {
                        // Drop publishes a final clear before closing the sender.
                        final_clear(&client, &endpoint, &receiver).await;
                        return;
                    }
                    current = receiver.borrow_and_update().clone();
                }
                result = post(&client,&endpoint,&snapshot) => {
                    if let Err(error) = result {
                        tracing::debug!(%error,"Lily response snapshot transport failed");
                        ready = false;
                        tokio::time::sleep(RETRY_DELAY).await;
                    }
                }
            }
        } else {
            if receiver.changed().await.is_err() {
                return;
            }
            current = receiver.borrow_and_update().clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::Mutex;
    use tokio::io::AsyncReadExt;
    use tokio::io::AsyncWriteExt;

    async fn server(
        health: &'static str,
    ) -> (
        String,
        Arc<Mutex<Vec<Snapshot>>>,
        tokio::task::JoinHandle<()>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let received = Arc::new(Mutex::new(Vec::new()));
        let output = received.clone();
        let task = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let received = output.clone();
                tokio::spawn(async move {
                    let mut bytes = Vec::new();
                    let (end, length) = loop {
                        let mut buf = [0_u8; 1024];
                        let count = socket.read(&mut buf).await.unwrap();
                        if count == 0 {
                            return;
                        }
                        bytes.extend_from_slice(&buf[..count]);
                        assert!(bytes.len() <= 16 * 1024);
                        if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                            let headers = std::str::from_utf8(&bytes[..end]).unwrap();
                            let length = headers
                                .lines()
                                .find_map(|l| {
                                    l.to_ascii_lowercase()
                                        .strip_prefix("content-length:")
                                        .map(|n| n.trim().parse::<usize>().unwrap())
                                })
                                .unwrap_or(0);
                            if bytes.len() >= end + 4 + length {
                                break (end + 4, length);
                            }
                        }
                    };
                    let first = std::str::from_utf8(&bytes[..end])
                        .unwrap()
                        .lines()
                        .next()
                        .unwrap();
                    let (status, body) = if first.starts_with("GET /health ") {
                        ("200 OK", health)
                    } else {
                        assert!(first.starts_with("POST /v1/snapshots "));
                        received
                            .lock()
                            .unwrap()
                            .push(serde_json::from_slice(&bytes[end..end + length]).unwrap());
                        ("202 Accepted", "{}")
                    };
                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                });
            }
        });
        (endpoint, received, task)
    }
    const READY: &str = r#"{"protocol":"lily2vec-v1","backend":"avx-vnni","ready":true}"#;
    async fn wait_count(received: &Arc<Mutex<Vec<Snapshot>>>, count: usize) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while received.lock().unwrap().len() < count {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }
    #[test]
    fn snapshots_are_bounded_verbatim_even_when_transport_skips_a_large_prefix() {
        for text in [
            "🙂愛 ".repeat(10_000),
            "word ".repeat(10_000),
            "x".repeat(10_000),
        ] {
            let tail = snapshot(&text);
            assert!(text.ends_with(&tail));
            assert!(tail.chars().count() <= RETAIN_CHARS);
        }
        let mut sampler = Sampler::default();
        assert!(sampler.push("old prefix", Duration::ZERO).is_none());
        let delta = "new words ".repeat(2000);
        let tail = sampler.push(&delta, Duration::ZERO).unwrap();
        assert!(delta.ends_with(&tail));
        assert!(!tail.contains("old prefix"));
    }
    #[test]
    fn sampling_coalesces_fast_text_and_finishes_short_responses() {
        let mut sampler = Sampler::default();
        assert!(sampler.push("hi", Duration::ZERO).is_none());
        assert_eq!(sampler.finish(Some("hi"), Duration::ZERO).unwrap(), "hi");
        assert!(
            sampler
                .push(&"hello ".repeat(1000), Duration::from_secs(1))
                .is_none()
        );
        assert!(sampler.pending.chars().count() <= RETAIN_CHARS);
        assert!(
            sampler
                .finish(Some("complete tail"), Duration::from_secs(1))
                .is_none()
        );
        assert_eq!(
            sampler
                .finish(Some("next answer"), Duration::from_secs(3))
                .unwrap(),
            "next answer"
        );
    }
    #[test]
    fn stale_text_expires_but_cancellation_is_always_deliverable() {
        let now = Instant::now();
        let mut message = Snapshot {
            source: "age-test".into(),
            sequence: 1,
            text: "obsolete response".into(),
            clear: false,
            queued_at: now - MAX_PENDING_AGE,
        };
        assert!(message.expired(now));
        message.clear = true;
        assert!(!message.expired(now));
        message.clear = false;
        message.queued_at = now;
        assert!(!message.expired(now));
        let json = serde_json::to_value(&message).unwrap();
        assert!(json.get("queued_at").is_none());
    }

    #[tokio::test]
    async fn health_checks_the_service_protocol_and_vnni_backend() {
        for health in [
            READY,
            r#"{"protocol":"other","backend":"avx-vnni","ready":true}"#,
            r#"{"protocol":"lily2vec-v1","backend":"scalar","ready":true}"#,
            r#"{"protocol":"lily2vec-v1","backend":"avx-vnni","ready":false}"#,
        ] {
            let (endpoint, _, server) = server(health).await;
            assert_eq!(
                health_ready(&client().unwrap(), &endpoint).await,
                health == READY
            );
            server.abort();
        }
    }
    #[tokio::test]
    async fn worker_only_sends_latest_snapshot_then_final_clear() {
        let (endpoint, received, server) = server(READY).await;
        let (sender, receiver) = watch::channel(None);
        // No yield until all writes complete: the bounded slot must contain
        // only the latest request even if readiness has not started yet.
        for sequence in 1..=500 {
            sender.send_replace(Some(Snapshot {
                source: "test-source".into(),
                sequence,
                text: format!("response {sequence}"),
                clear: false,
                queued_at: Instant::now(),
            }));
        }
        let task = tokio::spawn(worker(receiver, endpoint));
        wait_count(&received, 1).await;
        assert_eq!(received.lock().unwrap()[0].sequence, 500);
        sender.send_replace(Some(Snapshot {
            source: "test-source".into(),
            sequence: 501,
            text: String::new(),
            clear: true,
            queued_at: Instant::now(),
        }));
        drop(sender);
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap();
        let received = received.lock().unwrap();
        assert_eq!(received.len(), 2);
        assert!(received[1].clear);
        assert_eq!(received[1].sequence, 501);
        server.abort();
    }
    #[tokio::test]
    async fn a_closed_view_still_sends_final_clear_when_service_was_not_ready() {
        let (endpoint, received, server) = server(READY).await;
        let (sender, receiver) = watch::channel(Some(Snapshot {
            source: "closed-view".into(),
            sequence: 42,
            text: String::new(),
            clear: true,
            queued_at: Instant::now(),
        }));
        drop(sender);
        tokio::time::timeout(Duration::from_secs(3), worker(receiver, endpoint))
            .await
            .unwrap();
        let received = received.lock().unwrap();
        assert_eq!(received.len(), 1);
        assert!(received[0].clear);
        assert_eq!(received[0].sequence, 42);
        server.abort();
    }

    #[tokio::test]
    async fn dropping_the_view_cancels_its_pending_service_work() {
        let (endpoint, received, server) = server(READY).await;
        let (latest, receiver) = watch::channel(None);
        let mut view = ResponsePresentation {
            started: Instant::now(),
            sampler: Sampler::default(),
            source: "view".into(),
            sequence: 0,
            latest,
        };
        let task = tokio::spawn(worker(receiver, endpoint));
        view.finish(Some("hi"));
        wait_count(&received, 1).await;
        drop(view);
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap();
        let received = received.lock().unwrap();
        assert_eq!(received[0].text, "hi");
        assert!(received.last().unwrap().clear);
        assert!(received.last().unwrap().sequence > received[0].sequence);
        server.abort();
    }
}
