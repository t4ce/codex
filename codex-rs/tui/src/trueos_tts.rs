//! Native TRUEOS read-aloud for reasoning summaries and public replies.
//! Model inference and pronunciation belong to the speech Blueprint, not Codex.
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

use pulldown_cmark::Event;
use pulldown_cmark::Parser;
use pulldown_cmark::Tag;
use pulldown_cmark::TagEnd;
use serde::Deserialize;
use tokio::sync::mpsc;
use unicode_segmentation::UnicodeSegmentation;

const SERVICE_ENDPOINT: &str = "http://127.0.0.1:9773";
const SERVICE_PROTOCOL: &str = "ptts-v1";
const LAUNCH_COMMAND: &str = "launch --headless ptts\n";
const MAX_TEXT_BYTES: usize = 16 * 1024;
const OPENING_BUFFER_BYTES: usize = 1024;
const QUEUED_TEXT_WARNING_BYTES: usize = 1024;

/// Preserve prose and link labels; omit code blocks, raw HTML and image URLs.
/// Number expansion and pronunciation stay in Pocket TTS.
fn speech_text(markdown: &str) -> Option<String> {
    let mut out = String::new();
    let mut skip = 0usize;
    for event in Parser::new(markdown) {
        match event {
            Event::Start(Tag::CodeBlock(_) | Tag::Image { .. }) => skip += 1,
            Event::End(TagEnd::CodeBlock | TagEnd::Image) => {
                skip = skip.saturating_sub(1);
                out.push(' ');
            }
            _ if skip != 0 => {}
            Event::Text(text) | Event::Code(text) => out.push_str(&text),
            Event::SoftBreak
            | Event::HardBreak
            | Event::Rule
            | Event::End(TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::Item) => out.push(' '),
            _ => {}
        }
        if out.len() > MAX_TEXT_BYTES {
            return None;
        }
    }
    let text = out.split_whitespace().collect::<Vec<_>>().join(" ");
    (!text.is_empty()).then_some(text)
}

fn first_complete_sentence(text: &str, at_boundary: bool) -> Option<&str> {
    let sentence = text.split_sentence_bounds().next()?.trim_end();
    // A trailing period in a delta can still become part of a decimal or filename.
    let ends_with_punctuation = sentence
        .trim_end_matches(['"', '\'', '”', '’', ')', ']', '»'])
        .ends_with(['.', '!', '?', '。', '！', '？', '…']);
    (ends_with_punctuation && (at_boundary || sentence.len() < text.trim_end().len()))
        .then_some(sentence)
}

#[derive(Default)]
struct SummarySpeech {
    item_id: String,
    buffer: Option<String>,
}

impl SummarySpeech {
    fn sentence(&mut self, item_id: &str, delta: &str, at_boundary: bool) -> Option<String> {
        if self.item_id != item_id {
            self.item_id = item_id.to_owned();
            self.buffer = Some(String::new());
        }
        let buffer = self.buffer.as_mut()?;
        if delta.len() > MAX_TEXT_BYTES - buffer.len() {
            self.buffer = None;
            return None;
        }
        buffer.push_str(delta);

        let text = speech_text(buffer)?;
        let sentence = first_complete_sentence(&text, at_boundary)?;
        self.buffer = None;
        Some(sentence.to_owned())
    }

    fn finish(&mut self, item_id: &str, parts: &[String]) -> Option<String> {
        if self.item_id == item_id && self.buffer.is_none() {
            return None;
        }
        self.item_id = item_id.to_owned();
        self.buffer = None;
        parts.iter().find_map(|part| {
            let text = speech_text(part)?;
            first_complete_sentence(&text, /*at_boundary*/ true).map(str::to_owned)
        })
    }
}

struct AnswerOpening {
    turn: String,
    item: String,
    markdown: String,
    spoken: Option<String>,
}

impl AnswerOpening {
    fn push(&mut self, delta: &str) -> Option<String> {
        if self.spoken.is_some() || self.markdown.len() == OPENING_BUFFER_BYTES {
            return None;
        }
        let room = OPENING_BUFFER_BYTES - self.markdown.len();
        self.markdown
            .push_str(&delta[..delta.floor_char_boundary(room)]);
        let markdown = self.markdown.trim_start_matches(['\r', '\n']);
        let sentence = first_complete_sentence(markdown, /*at_boundary*/ false)?;
        // Only start early for plain prose. Incomplete Markdown (including links,
        // directives and code) can change meaning when later text arrives.
        if !markdown[sentence.len()..].starts_with(char::is_whitespace)
            || !sentence.starts_with(char::is_alphabetic)
            || sentence.contains(['\n', '\r', '`', '[', ']', '<', '>', '*', '_', '&', '\\'])
        {
            return None;
        }
        let text = speech_text(sentence)?;
        self.spoken = Some(text.clone());
        Some(text)
    }
}

#[derive(Default)]
pub(crate) struct ReadAloud {
    queue: Option<mpsc::UnboundedSender<String>>,
    pending_bytes: Arc<AtomicUsize>,
    last: Option<(String, String, String)>,
    summary: SummarySpeech,
    opening: Option<AnswerOpening>,
}
impl ReadAloud {
    #[cfg(test)]
    pub(crate) fn with_test_queue() -> (Self, mpsc::UnboundedReceiver<String>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (
            Self {
                queue: Some(tx),
                ..Self::default()
            },
            rx,
        )
    }

    pub(crate) fn new(enabled: bool) -> Self {
        let mut reader = Self::default();
        if enabled {
            reader.queue = Some(start_worker(Arc::clone(&reader.pending_bytes)));
        }
        reader
    }
    pub(crate) fn speak_summary(&mut self, item: &str, delta: &str, at_boundary: bool) {
        if let Some(sentence) = self.summary.sentence(item, delta, at_boundary) {
            self.enqueue(sentence);
        }
    }

    pub(crate) fn finish_summary(&mut self, item: &str, parts: &[String]) {
        if let Some(sentence) = self.summary.finish(item, parts) {
            self.enqueue(sentence);
        }
    }

    pub(crate) fn start_answer(&mut self, turn: &str, item: &str, text: &str) {
        if self
            .opening
            .as_ref()
            .is_some_and(|opening| opening.turn == turn && opening.item == item)
            || self
                .last
                .as_ref()
                .is_some_and(|(_, last_turn, last_item)| last_turn == turn && last_item == item)
        {
            return;
        }
        self.opening = Some(AnswerOpening {
            turn: turn.to_owned(),
            item: item.to_owned(),
            markdown: String::new(),
            spoken: None,
        });
        self.answer_delta(turn, item, text);
    }

    pub(crate) fn answer_delta(&mut self, turn: &str, item: &str, delta: &str) {
        if let Some(opening) = self.opening.as_mut()
            && opening.turn == turn
            && opening.item == item
            && let Some(sentence) = opening.push(delta)
        {
            self.enqueue(sentence);
        }
    }

    pub(crate) fn speak(&mut self, thread: &str, turn: &str, item: &str, text: &str) {
        let identity = (thread.to_owned(), turn.to_owned(), item.to_owned());
        if self.last.as_ref() == Some(&identity) {
            return;
        }
        self.last = Some(identity);
        let opening = self
            .opening
            .take_if(|opening| opening.turn == turn && opening.item == item);
        let Some(text) = speech_text(text) else {
            return;
        };
        if let Some(spoken) = opening.and_then(|opening| opening.spoken)
            && let Some(rest) = text.strip_prefix(&spoken)
            && (rest.is_empty() || rest.starts_with(' '))
        {
            if !rest.is_empty() {
                self.enqueue(rest.trim_start().to_owned());
            }
        } else {
            self.enqueue(text);
        }
    }

    fn enqueue(&mut self, text: String) {
        let pending_bytes = Arc::clone(&self.pending_bytes);
        let queue = self
            .queue
            .get_or_insert_with(|| start_worker(Arc::clone(&pending_bytes)));
        let text_bytes = text.len();
        let queued_bytes = pending_bytes.fetch_add(text_bytes, Ordering::Relaxed) + text_bytes;
        if queue.send(text).is_err() {
            pending_bytes.fetch_sub(text_bytes, Ordering::Relaxed);
            tracing::warn!(service = "ptts", "speech queue unavailable");
        } else if queued_bytes > QUEUED_TEXT_WARNING_BYTES {
            tracing::warn!(
                service = "ptts",
                queued_bytes,
                warning_bytes = QUEUED_TEXT_WARNING_BYTES,
                "speech queue exceeds 1 KiB; keep response text small"
            );
        }
    }
}
fn start_worker(pending_bytes: Arc<AtomicUsize>) -> mpsc::UnboundedSender<String> {
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(worker(rx, pending_bytes));
    tx
}
async fn next_speech(
    rx: &mut mpsc::UnboundedReceiver<String>,
    pending_bytes: &AtomicUsize,
) -> Option<String> {
    let text = rx.recv().await?;
    pending_bytes.fetch_sub(text.len(), Ordering::Relaxed);
    Some(text)
}

#[derive(Deserialize)]
struct Health {
    protocol: String,
    ready: bool,
    tts: bool,
}
impl Health {
    fn is_ready(&self) -> bool {
        self.protocol == SERVICE_PROTOCOL && self.ready && self.tts
    }
}
async fn ready(client: &reqwest::Client) -> bool {
    let Ok(response) = client
        .get(format!("{SERVICE_ENDPOINT}/health"))
        .timeout(Duration::from_millis(750))
        .send()
        .await
    else {
        return false;
    };
    if !response.status().is_success() {
        return false;
    }
    response
        .json::<Health>()
        .await
        .is_ok_and(|health| health.is_ready())
}
fn launch() -> Result<(), String> {
    #[cfg(target_os = "trueos")]
    return trueos::vshell::launch_with_destination(
        "apps",
        LAUNCH_COMMAND,
        trueos::vshell::LaunchDestination::Headless,
    )
    .map_err(|code| format!("ptts Blueprint launch ({code})"));
    #[cfg(not(target_os = "trueos"))]
    Err("ptts launch requires TRUEOS".into())
}
async fn ensure_service(client: &reqwest::Client) -> Result<(), String> {
    if ready(client).await {
        return Ok(());
    }
    launch()?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    loop {
        if ready(client).await {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("ptts readiness timed out".into());
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}
async fn worker(mut rx: mpsc::UnboundedReceiver<String>, pending_bytes: Arc<AtomicUsize>) {
    let client = match reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_millis(250))
        .timeout(Duration::from_secs(185))
        .build()
    {
        Ok(client) => client,
        Err(e) => {
            tracing::warn!(service = "ptts", "speech client: {e}");
            return;
        }
    };
    // Start speech alongside Lily and SQLite, independently of the first reply.
    if let Err(e) = ensure_service(&client).await {
        tracing::warn!(service = "ptts", "speech startup: {e}");
    }
    while let Some(text) = next_speech(&mut rx, &pending_bytes).await {
        if let Err(e) = ensure_service(&client).await {
            tracing::warn!(service = "ptts", "speech unavailable: {e}");
            continue;
        }
        // Submit FIFO entries once; never retry a possibly accepted POST.
        match client
            .post(format!("{SERVICE_ENDPOINT}/speech"))
            .json(&serde_json::json!({"text":text}))
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => {}
            Ok(response) => {
                tracing::warn!(service = "ptts", "speech rejected: {}", response.status())
            }
            Err(e) => tracing::warn!(service = "ptts", "speech request: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn queues_only_the_first_complete_sentence_per_summary_item() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut reader = ReadAloud {
            queue: Some(tx),
            ..ReadAloud::default()
        };

        reader.speak_summary("first", "The version is 1.", false);
        assert!(rx.try_recv().is_err());
        reader.speak_summary("first", "2. I will", false);
        assert_eq!(rx.try_recv().unwrap(), "The version is 1.2.");
        reader.speak_summary("first", " check the logs.", true);
        assert!(rx.try_recv().is_err());

        reader.speak_summary("second", "The logs are clear.", false);
        assert!(rx.try_recv().is_err());
        reader.speak_summary("second", "", true);
        reader.speak("thread", "turn", "reply", "Done.");
        assert_eq!(rx.try_recv().unwrap(), "The logs are clear.");
        assert_eq!(rx.try_recv().unwrap(), "Done.");

        reader.speak_summary("third", "An unfinished fragment", true);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn service_endpoint_protocol_and_launch_identity_stay_together() {
        assert_eq!(SERVICE_ENDPOINT, "http://127.0.0.1:9773");
        assert_eq!(SERVICE_PROTOCOL, "ptts-v1");
        assert_eq!(LAUNCH_COMMAND, "launch --headless ptts\n");
    }
    #[test]
    fn readiness_requires_pocket_protocol_and_a_ready_tts_model() {
        for protocol in ["ptts-v1", "other"] {
            for ready in [false, true] {
                for tts in [false, true] {
                    let health = Health {
                        protocol: protocol.into(),
                        ready,
                        tts,
                    };
                    assert_eq!(
                        health.is_ready(),
                        protocol == SERVICE_PROTOCOL && ready && tts
                    );
                }
            }
        }
    }
    #[test]
    fn speaks_prose_and_labels_without_code_or_url_destinations() {
        assert_eq!(
            speech_text(
                "**Hello** [friend](https://example.com).\n\n```rust\nsecret();\n```\nUse `cargo test`. ![image](file:///tmp/pic.png)"
            ),
            Some("Hello friend. Use cargo test.".into())
        );
    }
    #[test]
    fn refuses_empty_or_oversized_answers_without_truncating_meaning() {
        assert!(speech_text("```\ncode\n```").is_none());
        assert!(speech_text(&"é".repeat(MAX_TEXT_BYTES)).is_none());
    }
    #[tokio::test]
    async fn keeps_completion_order_and_counts_pending_utf8_bytes() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut reader = ReadAloud {
            queue: Some(tx),
            ..ReadAloud::default()
        };
        let pending_bytes = Arc::clone(&reader.pending_bytes);
        reader.speak("thread", "turn", "item", "Hello");
        reader.speak("thread", "turn", "item", "Hello again");
        reader.speak("thread", "next", "item", "Grüße");
        assert_eq!(pending_bytes.load(Ordering::Relaxed), "HelloGrüße".len());
        drop(reader);
        assert_eq!(
            next_speech(&mut rx, &pending_bytes).await.as_deref(),
            Some("Hello")
        );
        assert_eq!(pending_bytes.load(Ordering::Relaxed), "Grüße".len());
        assert_eq!(
            next_speech(&mut rx, &pending_bytes).await.as_deref(),
            Some("Grüße")
        );
        assert_eq!(pending_bytes.load(Ordering::Relaxed), 0);
        assert!(next_speech(&mut rx, &pending_bytes).await.is_none());
    }
    #[tokio::test]
    async fn warning_threshold_preserves_all_queued_text() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut reader = ReadAloud {
            queue: Some(tx),
            ..ReadAloud::default()
        };
        let first = "a".repeat(QUEUED_TEXT_WARNING_BYTES);
        reader.speak("thread", "first", "item", &first);
        reader.speak("thread", "next", "item", "é");
        assert_eq!(reader.pending_bytes.load(Ordering::Relaxed), 1026);
        assert_eq!(
            next_speech(&mut rx, &reader.pending_bytes).await.as_deref(),
            Some(first.as_str())
        );
        assert_eq!(
            next_speech(&mut rx, &reader.pending_bytes).await.as_deref(),
            Some("é")
        );
        assert_eq!(reader.pending_bytes.load(Ordering::Relaxed), 0);
    }
}
