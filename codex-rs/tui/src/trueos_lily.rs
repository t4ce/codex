//! Completed assistant turns sent to the independent Lily Blueprint VM.
//! The terminal only buffers bounded response text. The background transport
//! owns service readiness and submits each completed turn in order.
use serde::Deserialize;
use serde::Serialize;
use std::time::Duration;
use std::time::Instant;
use tokio::sync::mpsc;

const ENDPOINT: &str = "http://127.0.0.1:9771";
#[cfg(target_os = "trueos")]
const START_SCRIPT: &str = "launch --headless lily2vec\n";
const SOFT_CHARS: usize = 384;
const RETAIN_CHARS: usize = 768;
const PENDING_TURNS: usize = 32;
const MAX_PENDING_AGE: Duration = Duration::from_secs(30);
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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Turn {
    source: String,
    sequence: u64,
    text: String,
    #[serde(skip, default = "Instant::now")]
    queued_at: Instant,
}

pub(crate) struct ResponsePresentation {
    pending: String,
    finished: bool,
    source: String,
    sequence: u64,
    turns: mpsc::Sender<Turn>,
}

impl Default for ResponsePresentation {
    fn default() -> Self {
        let (turns, receiver) = mpsc::channel(PENDING_TURNS);
        tokio::spawn(worker(receiver, ENDPOINT.to_owned()));
        Self {
            pending: String::new(),
            finished: false,
            source: format!("codex-{}", uuid::Uuid::new_v4()),
            sequence: 0,
            turns,
        }
    }
}

impl ResponsePresentation {
    pub(crate) fn begin_turn(&mut self) {
        self.finished = false;
        self.clear();
    }

    pub(crate) fn delta(&mut self, text: &str) {
        let tail = snapshot(text);
        if tail.len() != text.len() {
            self.pending = tail;
        } else {
            self.pending.push_str(&tail);
            if self.pending.chars().count() > RETAIN_CHARS {
                self.pending = snapshot(&self.pending);
            }
        }
    }

    pub(crate) fn remember(&mut self, response: &str) {
        self.pending = snapshot(response);
    }

    pub(crate) fn finish(&mut self, response: Option<&str>) {
        if self.finished {
            return;
        }
        self.finished = true;
        let text = response
            .filter(|text| !text.trim().is_empty())
            .map(snapshot)
            .unwrap_or_else(|| std::mem::take(&mut self.pending));
        self.clear();
        if text.trim().is_empty() {
            return;
        }
        self.sequence = self.sequence.saturating_add(1);
        let turn = Turn {
            source: self.source.clone(),
            sequence: self.sequence,
            text,
            queued_at: Instant::now(),
        };
        if let Err(error) = self.turns.try_send(turn) {
            tracing::warn!(%error, sequence = self.sequence, "Lily completed turn was not queued");
        }
    }

    pub(crate) fn clear(&mut self) {
        self.pending.clear();
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
    health.ready && health.protocol == "lily2vec-v2" && health.backend == "avx-vnni"
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

async fn post(client: &reqwest::Client, endpoint: &str, turn: &Turn) -> Result<(), String> {
    let response = client
        .post(format!("{endpoint}/v1/turns"))
        .json(turn)
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if response.status() == reqwest::StatusCode::ACCEPTED {
        Ok(())
    } else {
        Err(format!("Lily turn rejected ({})", response.status()))
    }
}

async fn worker(mut receiver: mpsc::Receiver<Turn>, endpoint: String) {
    let client = match client() {
        Ok(client) => client,
        Err(error) => {
            tracing::warn!(%error, "Lily response transport unavailable");
            return;
        }
    };
    let mut ready = false;
    while let Some(turn) = receiver.recv().await {
        let mut last_error = None;
        while turn.queued_at.elapsed() < MAX_PENDING_AGE {
            if !ready {
                match ensure_service(&client, &endpoint).await {
                    Ok(()) => ready = true,
                    Err(error) => {
                        if last_error.as_ref() != Some(&error) {
                            tracing::warn!(%error, "Lily service unavailable");
                        }
                        last_error = Some(error);
                        tokio::time::sleep(RETRY_DELAY).await;
                        continue;
                    }
                }
            }
            match post(&client, &endpoint, &turn).await {
                Ok(()) => break,
                Err(error) => {
                    if last_error.as_ref() != Some(&error) {
                        tracing::warn!(%error, sequence = turn.sequence, "Lily turn transport failed");
                    }
                    last_error = Some(error);
                    ready = false;
                    tokio::time::sleep(RETRY_DELAY).await;
                }
            }
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
    ) -> (String, Arc<Mutex<Vec<Turn>>>, tokio::task::JoinHandle<()>) {
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
                        assert!(first.starts_with("POST /v1/turns "));
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
    const READY: &str = r#"{"protocol":"lily2vec-v2","backend":"avx-vnni","ready":true}"#;
    async fn wait_count(received: &Arc<Mutex<Vec<Turn>>>, count: usize) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while received.lock().unwrap().len() < count {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }
    #[test]
    fn completed_turns_are_bounded_and_only_sent_once() {
        let (turns, mut receiver) = mpsc::channel(PENDING_TURNS);
        let mut view = ResponsePresentation {
            pending: String::new(),
            finished: false,
            source: "view".into(),
            sequence: 0,
            turns,
        };
        for text in [
            "🙂愛 ".repeat(10_000),
            "word ".repeat(10_000),
            "x".repeat(10_000),
        ] {
            view.begin_turn();
            view.delta(&text);
            assert!(receiver.try_recv().is_err());
            view.finish(None);
            view.finish(Some("duplicate turn completion"));
            let turn = receiver.try_recv().unwrap();
            assert!(text.ends_with(&turn.text));
            assert!(turn.text.chars().count() <= RETAIN_CHARS);
            assert!(receiver.try_recv().is_err());
        }
        view.begin_turn();
        view.remember("final assistant item");
        view.finish(None);
        let turn = receiver.try_recv().unwrap();
        assert_eq!(turn.sequence, 4);
        assert_eq!(turn.text, "final assistant item");
        assert!(
            serde_json::to_value(&turn)
                .unwrap()
                .get("queued_at")
                .is_none()
        );
    }

    #[tokio::test]
    async fn health_checks_the_service_protocol_and_vnni_backend() {
        for health in [
            READY,
            r#"{"protocol":"other","backend":"avx-vnni","ready":true}"#,
            r#"{"protocol":"lily2vec-v2","backend":"scalar","ready":true}"#,
            r#"{"protocol":"lily2vec-v2","backend":"avx-vnni","ready":false}"#,
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
    async fn worker_delivers_completed_turns_in_order() {
        let (endpoint, received, server) = server(READY).await;
        let (sender, receiver) = mpsc::channel(PENDING_TURNS);
        for sequence in 1..=3 {
            sender
                .try_send(Turn {
                    source: "test-source".into(),
                    sequence,
                    text: format!("response {sequence}"),
                    queued_at: Instant::now(),
                })
                .unwrap();
        }
        drop(sender);
        let task = tokio::spawn(worker(receiver, endpoint));
        wait_count(&received, 3).await;
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap();
        let got = received.lock().unwrap();
        assert_eq!(
            got.iter().map(|turn| turn.sequence).collect::<Vec<_>>(),
            [1, 2, 3]
        );
        server.abort();
    }
}
