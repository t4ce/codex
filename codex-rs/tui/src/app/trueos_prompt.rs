//! Bounded, conversation-scoped inbox for the native Codex input grid.
//!
//! The reader owns each async filesystem operation through completion. A UI
//! event must never cancel a read after the kernel has handed over its text.

use std::future::Future;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::sync::watch;
use tokio::task::JoinHandle;

const POLL_INTERVAL: Duration = Duration::from_millis(200);
const RETRY_INTERVAL: Duration = Duration::from_secs(2);
const MAX_PROMPT_BYTES: usize = 16 * 1024;

struct Prompt {
    scope: String,
    text: String,
    accepted: oneshot::Sender<()>,
}

pub(super) struct PromptInbox {
    task: JoinHandle<()>,
    scope: watch::Sender<Option<String>>,
    receiver: mpsc::Receiver<Prompt>,
    pending: Option<Prompt>,
}

impl PromptInbox {
    #[cfg(target_os = "trueos")]
    pub(super) fn spawn() -> Self {
        Self::spawn_with_reader(|path| async move {
            trueos_v::vfs_async::read_file(path.as_bytes()).await
        })
    }

    fn spawn_with_reader<R, F>(mut read: R) -> Self
    where
        R: FnMut(String) -> F + Send + 'static,
        F: Future<Output = Result<Vec<u8>, i32>> + Send,
    {
        let (scope, mut scope_rx) = watch::channel::<Option<String>>(None);
        let (sender, receiver) = mpsc::channel(1);
        let task = tokio::spawn(async move {
            let mut last_error = None;
            loop {
                let active_scope = scope_rx.borrow_and_update().clone();
                let path = format!(
                    "vFile:codex-input/{}",
                    active_scope.as_deref().unwrap_or("offline")
                );
                // This await deliberately lives outside select!: it must not
                // lose a prompt when another UI event wakes the main loop.
                let result = read(path).await;
                let delay = match result {
                    Ok(bytes) => {
                        last_error = None;
                        if let Some(active_scope) = active_scope
                            && !bytes.is_empty()
                        {
                            match decode_prompt(bytes) {
                                Ok(text) => {
                                    let (accepted, acceptance) = oneshot::channel();
                                    if sender
                                        .send(Prompt {
                                            scope: active_scope,
                                            text,
                                            accepted,
                                        })
                                        .await
                                        .is_err()
                                    {
                                        return;
                                    }
                                    // At most one handed-over prompt exists.
                                    // While its original view is unavailable,
                                    // stop reading and let kernel readiness expire.
                                    if acceptance.await.is_err() {
                                        return;
                                    }
                                }
                                Err(()) => tracing::warn!("invalid native Codex input frame"),
                            }
                        }
                        POLL_INTERVAL
                    }
                    Err(code) => {
                        // Old kernels simply lack this virtual endpoint. Keep
                        // that optional integration quiet and retry slowly.
                        if last_error != Some(code) {
                            tracing::debug!(code, "native Codex input endpoint unavailable");
                            last_error = Some(code);
                        }
                        RETRY_INTERVAL
                    }
                };
                tokio::select! {
                    _ = tokio::time::sleep(delay) => {},
                    result = scope_rx.changed() => {
                        if result.is_err() { return; }
                    },
                }
            }
        });
        Self {
            task,
            scope,
            receiver,
            pending: None,
        }
    }

    pub(super) fn set_scope(&self, scope: Option<String>) {
        if *self.scope.borrow() != scope {
            self.scope.send_replace(scope);
        }
    }

    pub(super) fn can_receive(&self) -> bool {
        self.pending.is_none()
    }

    pub(super) async fn receive(&mut self) {
        match self.receiver.recv().await {
            Some(prompt) => self.pending = Some(prompt),
            None => std::future::pending().await,
        }
    }

    pub(super) fn try_deliver(&mut self, scope: Option<&str>, submit: impl FnOnce(String) -> bool) {
        let Some(prompt) = self.pending.as_ref() else {
            return;
        };
        if scope == Some(prompt.scope.as_str()) && submit(prompt.text.clone()) {
            let prompt = self.pending.take().expect("matched pending prompt");
            let _ = prompt.accepted.send(());
        }
    }
}

impl Drop for PromptInbox {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn decode_prompt(bytes: Vec<u8>) -> Result<String, ()> {
    if bytes.len() > MAX_PROMPT_BYTES {
        return Err(());
    }
    let text = String::from_utf8(bytes).map_err(|_| ())?;
    if text.trim().is_empty() || text.contains('\0') {
        return Err(());
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::Notify;

    fn inbox_with_text(text: &str) -> (PromptInbox, mpsc::UnboundedReceiver<String>) {
        let (paths, paths_rx) = mpsc::unbounded_channel();
        let mut text = Some(text.as_bytes().to_vec());
        let inbox = PromptInbox::spawn_with_reader(move |path| {
            let _ = paths.send(path);
            let bytes = text.take().unwrap_or_default();
            async move { Ok(bytes) }
        });
        (inbox, paths_rx)
    }

    #[test]
    fn one_utf8_statement_retains_its_lines_and_prefix() {
        let text = "!this is user text\n第二行\n/status\n";
        assert_eq!(decode_prompt(text.as_bytes().to_vec()), Ok(text.into()));
    }

    #[test]
    fn malformed_or_empty_frames_are_rejected_without_truncation() {
        for bytes in [
            vec![b'x'; MAX_PROMPT_BYTES + 1],
            vec![0xff],
            b" \n\t".to_vec(),
            b"some\0text".to_vec(),
        ] {
            assert!(decode_prompt(bytes).is_err());
        }
        assert_eq!(
            decode_prompt(vec![b'x'; MAX_PROMPT_BYTES]).unwrap().len(),
            MAX_PROMPT_BYTES
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_pending_statement_is_delivered_once_and_stops_further_reads() {
        let (mut inbox, mut paths) = inbox_with_text("first\nsecond");
        inbox.set_scope(Some("main".into()));
        inbox.receive().await;
        assert_eq!(paths.recv().await.unwrap(), "vFile:codex-input/main");
        tokio::time::advance(Duration::from_secs(5)).await;
        assert!(paths.try_recv().is_err());
        assert!(!inbox.can_receive());
        let mut delivered = Vec::new();
        inbox.try_deliver(Some("main"), |text| {
            delivered.push(text);
            true
        });
        inbox.try_deliver(Some("main"), |_| panic!("duplicate delivery"));
        assert_eq!(delivered, ["first\nsecond"]);
        assert!(inbox.can_receive());
        tokio::task::yield_now().await;
        tokio::time::advance(POLL_INTERVAL).await;
        assert_eq!(paths.recv().await.unwrap(), "vFile:codex-input/main");
    }

    #[tokio::test(start_paused = true)]
    async fn scope_changes_never_retarget_text_and_original_view_can_resume() {
        let (mut inbox, mut paths) = inbox_with_text("for original");
        inbox.set_scope(Some("original".into()));
        inbox.receive().await;
        paths.recv().await.unwrap();
        inbox.set_scope(Some("other".into()));
        inbox.try_deliver(Some("other"), |_| panic!("wrong conversation"));
        inbox.try_deliver(None, |_| panic!("blocked view"));
        tokio::time::advance(Duration::from_secs(5)).await;
        assert!(paths.try_recv().is_err());
        inbox.set_scope(Some("original".into()));
        inbox.try_deliver(Some("original"), |text| text == "for original");
        assert!(inbox.can_receive());
    }

    #[tokio::test]
    async fn rejected_submission_remains_owned_until_acceptance() {
        let (mut inbox, _) = inbox_with_text("keep this");
        inbox.set_scope(Some("main".into()));
        inbox.receive().await;
        inbox.try_deliver(Some("main"), |text| {
            assert_eq!(text, "keep this");
            false
        });
        assert!(!inbox.can_receive());
        inbox.try_deliver(Some("main"), |text| text == "keep this");
        assert!(inbox.can_receive());
    }

    #[tokio::test(start_paused = true)]
    async fn cancelling_ui_receive_does_not_cancel_an_inflight_filesystem_read() {
        let finish = Arc::new(Notify::new());
        let read_finish = Arc::clone(&finish);
        let (started, mut starts) = mpsc::unbounded_channel();
        let mut inbox = PromptInbox::spawn_with_reader(move |path| {
            let finish = Arc::clone(&read_finish);
            let _ = started.send(path);
            async move {
                finish.notified().await;
                Ok(b"still here".to_vec())
            }
        });
        inbox.set_scope(Some("main".into()));
        assert_eq!(starts.recv().await.unwrap(), "vFile:codex-input/main");
        assert!(
            tokio::time::timeout(POLL_INTERVAL, inbox.receive())
                .await
                .is_err()
        );
        finish.notify_one();
        inbox.receive().await;
        inbox.try_deliver(Some("main"), |text| text == "still here");
        assert!(inbox.can_receive());
    }

    #[tokio::test(start_paused = true)]
    async fn old_kernel_errors_back_off_without_stopping_the_cli() {
        let (paths, mut paths_rx) = mpsc::unbounded_channel();
        let inbox = PromptInbox::spawn_with_reader(move |path| {
            let _ = paths.send(path);
            async { Err(-8) }
        });
        assert_eq!(paths_rx.recv().await.unwrap(), "vFile:codex-input/offline");
        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(paths_rx.try_recv().is_err());
        tokio::time::advance(Duration::from_secs(1)).await;
        assert_eq!(paths_rx.recv().await.unwrap(), "vFile:codex-input/offline");
        inbox.set_scope(Some("new".into()));
        assert_eq!(paths_rx.recv().await.unwrap(), "vFile:codex-input/new");
        inbox.set_scope(None);
        assert_eq!(paths_rx.recv().await.unwrap(), "vFile:codex-input/offline");
    }
}
