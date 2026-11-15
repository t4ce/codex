//! Opt-in local read-aloud. Only completed public final answers enter here.
//! Model inference and lexicalization belong to the ttstt Blueprint, not Codex.
use std::time::Duration;
use std::time::Instant;

use pulldown_cmark::Event;
use pulldown_cmark::Parser;
use pulldown_cmark::Tag;
use pulldown_cmark::TagEnd;
use serde::Deserialize;
use tokio::sync::watch;

const ENDPOINT: &str = "http://127.0.0.1:9772";
const MAX_TEXT_BYTES: usize = 16 * 1024;
const MAX_AGE: Duration = Duration::from_secs(30);

/// Preserve prose and link labels; omit code blocks, raw HTML and image URLs.
/// Number expansion and pronunciation stay in Kokoro's lexicon/G2P frontend.
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

#[derive(Clone)]
struct Speech {
    text: String,
    queued: Instant,
}
#[derive(Default)]
pub(crate) struct ReadAloud {
    latest: Option<watch::Sender<Option<Speech>>>,
    last: Option<(String, String, String)>,
}
impl ReadAloud {
    pub(crate) fn speak(&mut self, thread: &str, turn: &str, item: &str, text: &str) {
        let identity = (thread.to_owned(), turn.to_owned(), item.to_owned());
        if self.last.as_ref() == Some(&identity) {
            return;
        }
        self.last = Some(identity);
        let Some(text) = speech_text(text) else {
            return;
        };
        let latest = self.latest.get_or_insert_with(|| {
            let (tx, rx) = watch::channel(None);
            tokio::spawn(worker(rx));
            tx
        });
        latest.send_replace(Some(Speech {
            text,
            queued: Instant::now(),
        }));
    }
    // Clearing a pending answer does not abort the HTTP request already playing
    // in the service. In-flight interruption is a separate future protocol step.
    pub(crate) fn clear(&mut self) {
        if let Some(latest) = &self.latest {
            latest.send_replace(None);
        }
    }
}
impl Drop for ReadAloud {
    fn drop(&mut self) {
        self.clear();
    }
}

#[derive(Deserialize)]
struct Health {
    protocol: String,
    ready: bool,
    tts: bool,
}
async fn ready(client: &reqwest::Client) -> bool {
    let Ok(response) = client
        .get(format!("{ENDPOINT}/health"))
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
        .is_ok_and(|h| h.protocol == "ttstt-v1" && h.ready && h.tts)
}
fn launch() -> Result<(), String> {
    #[cfg(target_os = "trueos")]
    return trueos::vshell::launch_with_destination(
        "apps",
        "launch --headless ttstt\n",
        trueos::vshell::LaunchDestination::Headless,
    )
    .map_err(|code| format!("ttstt Blueprint launch ({code})"));
    #[cfg(not(target_os = "trueos"))]
    Err("ttstt launch requires TRUEOS".into())
}
async fn worker(mut rx: watch::Receiver<Option<Speech>>) {
    let client = match reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_millis(250))
        .timeout(Duration::from_secs(185))
        .build()
    {
        Ok(client) => client,
        Err(e) => {
            tracing::warn!("ttstt client: {e}");
            return;
        }
    };
    while rx.changed().await.is_ok() {
        let Some(speech) = rx.borrow_and_update().clone() else {
            continue;
        };
        if speech.queued.elapsed() > MAX_AGE {
            continue;
        }
        if !ready(&client).await {
            if let Err(e) = launch() {
                tracing::warn!("ttstt unavailable: {e}");
                continue;
            }
            let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
            while !ready(&client).await && tokio::time::Instant::now() < deadline {
                if rx.has_changed().unwrap_or(true) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            if !ready(&client).await {
                tracing::warn!("ttstt readiness timed out");
                continue;
            }
        }
        // Pending answers may be replaced/cleared during service startup. Never
        // speak that stale snapshot, and never retry a possibly accepted POST.
        // An answer fresh when selected may wait through a cold model load.
        if rx.has_changed().unwrap_or(true) {
            continue;
        }
        match client
            .post(format!("{ENDPOINT}/speech"))
            .json(&serde_json::json!({"text":speech.text}))
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => {}
            Ok(response) => tracing::warn!("ttstt speech rejected: {}", response.status()),
            Err(e) => tracing::warn!("ttstt speech request: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
    async fn deduplicates_completion_and_clears_pending_answers() {
        let (tx, mut rx) = watch::channel(None);
        let mut reader = ReadAloud {
            latest: Some(tx),
            last: None,
        };
        reader.speak("thread", "turn", "item", "Hello");
        assert_eq!(rx.borrow_and_update().as_ref().unwrap().text, "Hello");
        reader.speak("thread", "turn", "item", "Hello again");
        assert!(!rx.has_changed().unwrap());
        reader.clear();
        assert!(rx.borrow_and_update().is_none());
        reader.speak("thread", "next", "item", "Next answer");
        assert_eq!(rx.borrow_and_update().as_ref().unwrap().text, "Next answer");
    }
}
