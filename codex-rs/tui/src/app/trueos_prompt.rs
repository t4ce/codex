//! Bounded, conversation-scoped inbox for the native Codex input grid.
//!
//! The reader owns each async filesystem operation through completion. A UI
//! event must never cancel a read after the kernel has handed over its text.
//! A pending statement keeps its logical backend and conversation identity even when another
//! connection exposes the same thread UUID. A v2 scope includes an opaque backend token;
//! legacy kernels are supported only for a pinned, genuinely single-backend run.

use crate::backend_id::BackendId;
use std::collections::HashMap;
use std::future::Future;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::sync::watch;
use tokio::task::JoinHandle;

const POLL_INTERVAL: Duration = Duration::from_millis(200);
const RETRY_INTERVAL: Duration = Duration::from_secs(2);
const MAX_PROMPT_BYTES: usize = 16 * 1024;
const OFFLINE_PATH: &str = "vFile:codex-input/offline";
// The native filesystem ABI returns BAD_PARAM for an unknown v2 scope on v1 kernels.
const FS_ERR_BAD_PARAM: i32 = -4;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PromptScope {
    backend_id: BackendId,
    thread_id: String,
}

impl PromptScope {
    pub(super) fn new(backend_id: BackendId, thread_id: String) -> Self {
        Self {
            backend_id,
            thread_id,
        }
    }
}

struct Prompt {
    scope: PromptScope,
    text: String,
    accepted: oneshot::Sender<()>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct LegacyEligibility {
    owner: Option<BackendId>,
    revoked: bool,
}

impl LegacyEligibility {
    fn observe(&mut self, eligible_owner: Option<BackendId>) {
        if self.revoked {
            return;
        }
        match (self.owner, eligible_owner) {
            (None, Some(owner)) => self.owner = Some(owner),
            (Some(owner), Some(current)) if owner == current => {}
            _ => self.revoked = true,
        }
    }

    fn allows(self, scope: &PromptScope) -> bool {
        !self.revoked && self.owner == Some(scope.backend_id)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct ReadContext {
    scope: Option<PromptScope>,
    legacy: LegacyEligibility,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Protocol {
    Probe,
    Qualified,
    Legacy,
}

fn qualified_path(scope: &PromptScope, tokens: &mut HashMap<BackendId, uuid::Uuid>) -> String {
    // The token is local to this inbox. Logical BackendId itself stays out of
    // remote protocols, and reconnect/request-only views reuse the same token.
    let token = tokens
        .entry(scope.backend_id)
        .or_insert_with(uuid::Uuid::new_v4);
    format!("vFile:codex-input/v2/{token}/{}", scope.thread_id)
}

pub(super) struct PromptInbox {
    task: JoinHandle<()>,
    scope: Option<watch::Sender<ReadContext>>,
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
        let (scope, mut scope_rx) = watch::channel(ReadContext::default());
        let (sender, receiver) = mpsc::channel(1);
        let task = tokio::spawn(async move {
            let mut last_error = None;
            let mut protocol = Protocol::Probe;
            let mut backend_tokens = HashMap::new();
            loop {
                let context = scope_rx.borrow_and_update().clone();
                let retire_legacy = protocol == Protocol::Legacy && context.legacy.revoked;
                if retire_legacy {
                    protocol = Protocol::Probe;
                }
                let active_scope = (!retire_legacy).then_some(context.scope).flatten();
                let qualified = active_scope.is_some() && protocol != Protocol::Legacy;
                let path = match active_scope.as_ref() {
                    Some(scope) if protocol == Protocol::Legacy && context.legacy.allows(scope) => {
                        format!("vFile:codex-input/{}", scope.thread_id)
                    }
                    Some(scope) if protocol != Protocol::Legacy => {
                        qualified_path(scope, &mut backend_tokens)
                    }
                    _ => OFFLINE_PATH.to_string(),
                };
                // This await deliberately lives outside select!: it must not
                // lose a prompt when another UI event wakes the main loop.
                let result = read(path).await;
                if scope_rx.has_changed().is_err() {
                    return;
                }
                let delay = match result {
                    Ok(bytes) => {
                        last_error = None;
                        if qualified {
                            protocol = Protocol::Qualified;
                        }
                        if let Some(active_scope) = active_scope
                            && !bytes.is_empty()
                        {
                            match decode_prompt(bytes) {
                                Ok(text) => {
                                    let (accepted, mut acceptance) = oneshot::channel();
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
                                    loop {
                                        tokio::select! {
                                            result = &mut acceptance => {
                                                if result.is_err() { return; }
                                                break;
                                            },
                                            changed = scope_rx.changed() => {
                                                if changed.is_err() { return; }
                                                // Revoke readiness even while a handed-over
                                                // statement waits for its original view.
                                                let revoked = scope_rx.borrow().legacy.revoked;
                                                if protocol == Protocol::Legacy && revoked {
                                                    let _ = read(OFFLINE_PATH.to_string()).await;
                                                    protocol = Protocol::Probe;
                                                }
                                            },
                                        }
                                    }
                                }
                                Err(()) => tracing::warn!("invalid native Codex input frame"),
                            }
                        }
                        POLL_INTERVAL
                    }
                    Err(code) => {
                        if qualified && protocol == Protocol::Probe && code == FS_ERR_BAD_PARAM {
                            if active_scope
                                .as_ref()
                                .is_some_and(|scope| context.legacy.allows(scope))
                            {
                                protocol = Protocol::Legacy;
                                continue;
                            }
                            // An old kernel must not keep a stale v1 receiver
                            // advertised when this run cannot use v1 safely.
                            let _ = read(OFFLINE_PATH.to_string()).await;
                        }
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
            scope: Some(scope),
            receiver,
            pending: None,
        }
    }

    pub(super) fn set_scope(
        &self,
        scope: Option<PromptScope>,
        legacy_eligible_owner: Option<BackendId>,
    ) {
        if let Some(sender) = self.scope.as_ref() {
            let mut context = sender.borrow().clone();
            context.scope = scope;
            // Record revocation here, before watch updates can coalesce while
            // the reader owns an in-flight read or waits for UI acceptance.
            context.legacy.observe(legacy_eligible_owner);
            if let Some(scope) = context.scope.as_ref()
                && context
                    .legacy
                    .owner
                    .is_some_and(|owner| owner != scope.backend_id)
            {
                context.legacy.revoked = true;
            }
            if *sender.borrow() != context {
                sender.send_replace(context);
            }
        }
    }

    pub(super) async fn shutdown(mut self) {
        // Closing the watch stops future reads, and wakes a reader waiting for
        // UI acceptance. An already-started VFS operation finishes first.
        self.scope.take();
        let _ = (&mut self.task).await;
        #[cfg(target_os = "trueos")]
        {
            // Retire host readiness before the embedded server begins its
            // potentially longer shutdown, while this VM is still running.
            let _ = tokio::time::timeout(
                RETRY_INTERVAL,
                trueos_v::vfs_async::read_file(OFFLINE_PATH.as_bytes()),
            )
            .await;
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

    pub(super) fn try_deliver(
        &mut self,
        scope: Option<&PromptScope>,
        submit: impl FnOnce(String) -> bool,
    ) {
        let Some(prompt) = self.pending.as_ref() else {
            return;
        };
        if scope == Some(&prompt.scope) && submit(prompt.text.clone()) {
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

    fn scope(thread_id: &str) -> PromptScope {
        static OWNER: std::sync::OnceLock<BackendId> = std::sync::OnceLock::new();
        PromptScope::new(*OWNER.get_or_init(BackendId::new), thread_id.into())
    }

    fn assert_scope_path(path: String, thread_id: &str) {
        if thread_id == "offline" {
            assert_eq!(path, OFFLINE_PATH);
            return;
        }
        let (backend, conversation) = path
            .strip_prefix("vFile:codex-input/v2/")
            .expect("qualified native prompt scope")
            .split_once('/')
            .expect("backend and conversation");
        assert!(uuid::Uuid::parse_str(backend).is_ok());
        assert_eq!(conversation, thread_id);
    }

    fn legacy_kernel(text: Option<&str>) -> (PromptInbox, mpsc::UnboundedReceiver<String>) {
        let (paths, paths_rx) = mpsc::unbounded_channel();
        let mut text = text.map(|text| text.as_bytes().to_vec());
        let inbox = PromptInbox::spawn_with_reader(move |path| {
            let _ = paths.send(path.clone());
            let result = if path.starts_with("vFile:codex-input/v2/") {
                Err(FS_ERR_BAD_PARAM)
            } else if path == OFFLINE_PATH {
                Ok(Vec::new())
            } else {
                Ok(text.take().unwrap_or_default())
            };
            async move { result }
        });
        (inbox, paths_rx)
    }

    #[test]
    fn qualified_backend_tokens_are_stable_and_new_conversations_are_scoped() {
        let owner = BackendId::new();
        let original = PromptScope::new(owner, "same-thread".into());
        let other = PromptScope::new(BackendId::new(), "same-thread".into());
        let mut tokens = HashMap::new();
        let path = qualified_path(&original, &mut tokens);
        assert_eq!(qualified_path(&original.clone(), &mut tokens), path);
        assert_ne!(qualified_path(&other, &mut tokens), path);
        let new_path = qualified_path(&PromptScope::new(owner, "new".into()), &mut tokens);
        assert_eq!(
            path.rsplit_once('/').unwrap().0,
            new_path.rsplit_once('/').unwrap().0
        );
        assert_scope_path(new_path, "new");
    }

    #[tokio::test(start_paused = true)]
    async fn old_kernel_fallback_requires_a_pinned_eligible_owner() {
        let original = scope("new");
        let (inbox, mut paths) = legacy_kernel(None);
        inbox.set_scope(Some(original.clone()), Some(original.backend_id));
        assert_scope_path(paths.recv().await.unwrap(), "new");
        assert_eq!(paths.recv().await.unwrap(), "vFile:codex-input/new");
        tokio::time::advance(POLL_INTERVAL).await;
        assert_eq!(paths.recv().await.unwrap(), "vFile:codex-input/new");
    }

    #[tokio::test(start_paused = true)]
    async fn old_kernel_never_uses_legacy_without_single_backend_eligibility() {
        let (inbox, mut paths) = legacy_kernel(None);
        inbox.set_scope(Some(scope("main")), None);
        assert_scope_path(paths.recv().await.unwrap(), "main");
        assert_eq!(paths.recv().await.unwrap(), OFFLINE_PATH);
        tokio::time::advance(RETRY_INTERVAL).await;
        assert_scope_path(paths.recv().await.unwrap(), "main");
        assert_eq!(paths.recv().await.unwrap(), OFFLINE_PATH);
    }

    #[tokio::test(start_paused = true)]
    async fn successful_v2_poll_never_downgrades_after_later_bad_param() {
        let (paths, mut paths_rx) = mpsc::unbounded_channel();
        let mut first = true;
        let inbox = PromptInbox::spawn_with_reader(move |path| {
            let _ = paths.send(path);
            let result = if std::mem::take(&mut first) {
                Ok(Vec::new())
            } else {
                Err(FS_ERR_BAD_PARAM)
            };
            async move { result }
        });
        let original = scope("main");
        inbox.set_scope(Some(original.clone()), Some(original.backend_id));
        assert_scope_path(paths_rx.recv().await.unwrap(), "main");
        tokio::time::advance(POLL_INTERVAL).await;
        assert_scope_path(paths_rx.recv().await.unwrap(), "main");
        tokio::time::advance(RETRY_INTERVAL).await;
        assert_scope_path(paths_rx.recv().await.unwrap(), "main");
    }

    #[tokio::test(start_paused = true)]
    async fn coalesced_policy_changes_permanently_revoke_legacy_fallback() {
        let original = scope("main");
        let (inbox, mut paths) = legacy_kernel(None);
        inbox.set_scope(Some(original.clone()), Some(original.backend_id));
        assert_scope_path(paths.recv().await.unwrap(), "main");
        assert_eq!(paths.recv().await.unwrap(), "vFile:codex-input/main");
        inbox.set_scope(Some(original.clone()), None);
        inbox.set_scope(Some(original.clone()), Some(original.backend_id));
        assert_eq!(paths.recv().await.unwrap(), OFFLINE_PATH);
        tokio::time::advance(POLL_INTERVAL).await;
        assert_scope_path(paths.recv().await.unwrap(), "main");
        assert_eq!(paths.recv().await.unwrap(), OFFLINE_PATH);
    }

    #[tokio::test(start_paused = true)]
    async fn owner_change_with_identical_thread_revokes_legacy_permanently() {
        let original = scope("same-thread");
        let other = PromptScope::new(BackendId::new(), "same-thread".into());
        let (inbox, mut paths) = legacy_kernel(None);
        inbox.set_scope(Some(original.clone()), Some(original.backend_id));
        assert_scope_path(paths.recv().await.unwrap(), "same-thread");
        assert_eq!(paths.recv().await.unwrap(), "vFile:codex-input/same-thread");
        inbox.set_scope(Some(other.clone()), Some(other.backend_id));
        inbox.set_scope(Some(original.clone()), Some(original.backend_id));
        assert_eq!(paths.recv().await.unwrap(), OFFLINE_PATH);
        tokio::time::advance(POLL_INTERVAL).await;
        assert_scope_path(paths.recv().await.unwrap(), "same-thread");
        assert_eq!(paths.recv().await.unwrap(), OFFLINE_PATH);
    }

    #[tokio::test]
    async fn legacy_revocation_retires_readiness_while_text_waits_for_acceptance() {
        let original = scope("main");
        let (mut inbox, mut paths) = legacy_kernel(Some("accepted for original"));
        inbox.set_scope(Some(original.clone()), Some(original.backend_id));
        inbox.receive().await;
        assert_scope_path(paths.recv().await.unwrap(), "main");
        assert_eq!(paths.recv().await.unwrap(), "vFile:codex-input/main");
        inbox.set_scope(Some(original.clone()), None);
        inbox.set_scope(Some(original.clone()), Some(original.backend_id));
        assert_eq!(paths.recv().await.unwrap(), OFFLINE_PATH);
        inbox.try_deliver(Some(&original), |text| text == "accepted for original");
        assert!(inbox.can_receive());
    }

    #[tokio::test]
    async fn inflight_legacy_read_cannot_erase_revocation_or_change_text_ownership() {
        let original = scope("same-thread");
        let other = PromptScope::new(BackendId::new(), "same-thread".into());
        let finish = Arc::new(Notify::new());
        let read_finish = Arc::clone(&finish);
        let (paths, mut paths_rx) = mpsc::unbounded_channel();
        let mut inbox = PromptInbox::spawn_with_reader(move |path| {
            let finish = Arc::clone(&read_finish);
            let _ = paths.send(path.clone());
            async move {
                if path.starts_with("vFile:codex-input/v2/") {
                    Err(FS_ERR_BAD_PARAM)
                } else if path == OFFLINE_PATH {
                    Ok(Vec::new())
                } else {
                    finish.notified().await;
                    Ok(b"original backend only".to_vec())
                }
            }
        });
        inbox.set_scope(Some(original.clone()), Some(original.backend_id));
        assert_scope_path(paths_rx.recv().await.unwrap(), "same-thread");
        assert_eq!(
            paths_rx.recv().await.unwrap(),
            "vFile:codex-input/same-thread"
        );
        inbox.set_scope(Some(other.clone()), Some(other.backend_id));
        inbox.set_scope(Some(original.clone()), Some(original.backend_id));
        finish.notify_one();
        inbox.receive().await;
        assert_eq!(paths_rx.recv().await.unwrap(), OFFLINE_PATH);
        inbox.try_deliver(Some(&other), |_| panic!("inflight text changed backend"));
        inbox.try_deliver(Some(&original), |text| text == "original backend only");
        assert!(inbox.can_receive());
    }

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
        inbox.set_scope(Some(scope("main")), None);
        inbox.receive().await;
        assert_scope_path(paths.recv().await.unwrap(), "main");
        tokio::time::advance(Duration::from_secs(5)).await;
        assert!(paths.try_recv().is_err());
        assert!(!inbox.can_receive());
        let mut delivered = Vec::new();
        inbox.try_deliver(Some(&scope("main")), |text| {
            delivered.push(text);
            true
        });
        inbox.try_deliver(Some(&scope("main")), |_| panic!("duplicate delivery"));
        assert_eq!(delivered, ["first\nsecond"]);
        assert!(inbox.can_receive());
        tokio::task::yield_now().await;
        tokio::time::advance(POLL_INTERVAL).await;
        assert_scope_path(paths.recv().await.unwrap(), "main");
    }

    #[tokio::test(start_paused = true)]
    async fn scope_changes_never_retarget_text_and_original_view_can_resume() {
        let (mut inbox, mut paths) = inbox_with_text("for original");
        inbox.set_scope(Some(scope("original")), None);
        inbox.receive().await;
        paths.recv().await.unwrap();
        inbox.set_scope(Some(scope("other")), None);
        inbox.try_deliver(Some(&scope("other")), |_| panic!("wrong conversation"));
        inbox.try_deliver(None, |_| panic!("blocked view"));
        tokio::time::advance(Duration::from_secs(5)).await;
        assert!(paths.try_recv().is_err());
        inbox.set_scope(Some(scope("original")), None);
        inbox.try_deliver(Some(&scope("original")), |text| text == "for original");
        assert!(inbox.can_receive());
    }

    #[tokio::test(start_paused = true)]
    async fn identical_thread_ids_on_another_backend_never_accept_a_pending_statement() {
        let original = PromptScope::new(BackendId::new(), "same-thread".into());
        let other = PromptScope::new(BackendId::new(), "same-thread".into());
        let (mut inbox, mut paths) = inbox_with_text("original backend only");
        inbox.set_scope(Some(original.clone()), None);
        inbox.receive().await;
        assert_scope_path(paths.recv().await.unwrap(), "same-thread");

        inbox.set_scope(Some(other.clone()), None);
        inbox.try_deliver(Some(&other), |_| panic!("wrong backend"));
        assert!(!inbox.can_receive());
        tokio::time::advance(Duration::from_secs(5)).await;
        assert!(paths.try_recv().is_err());

        inbox.set_scope(Some(original.clone()), None);
        inbox.try_deliver(Some(&original), |text| text == "original backend only");
        assert!(inbox.can_receive());
    }

    #[tokio::test]
    async fn an_inflight_read_keeps_its_original_backend_when_the_same_thread_is_selected_elsewhere()
     {
        let original = PromptScope::new(BackendId::new(), "same-thread".into());
        let other = PromptScope::new(BackendId::new(), "same-thread".into());
        let finish = Arc::new(Notify::new());
        let read_finish = Arc::clone(&finish);
        let (started, mut starts) = mpsc::unbounded_channel();
        let mut inbox = PromptInbox::spawn_with_reader(move |path| {
            let finish = Arc::clone(&read_finish);
            let _ = started.send(path);
            async move {
                finish.notified().await;
                Ok(b"read for original backend".to_vec())
            }
        });
        inbox.set_scope(Some(original.clone()), None);
        assert_scope_path(starts.recv().await.unwrap(), "same-thread");
        inbox.set_scope(Some(other.clone()), None);
        finish.notify_one();
        inbox.receive().await;
        inbox.try_deliver(Some(&other), |_| panic!("inflight read retargeted"));
        assert!(!inbox.can_receive());
        inbox.set_scope(Some(original.clone()), None);
        inbox.try_deliver(Some(&original), |text| text == "read for original backend");
        assert!(inbox.can_receive());
    }

    #[tokio::test]
    async fn rejected_submission_remains_owned_until_acceptance() {
        let (mut inbox, _) = inbox_with_text("keep this");
        inbox.set_scope(Some(scope("main")), None);
        inbox.receive().await;
        inbox.try_deliver(Some(&scope("main")), |text| {
            assert_eq!(text, "keep this");
            false
        });
        assert!(!inbox.can_receive());
        inbox.try_deliver(Some(&scope("main")), |text| text == "keep this");
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
        inbox.set_scope(Some(scope("main")), None);
        assert_scope_path(starts.recv().await.unwrap(), "main");
        assert!(
            tokio::time::timeout(POLL_INTERVAL, inbox.receive())
                .await
                .is_err()
        );
        finish.notify_one();
        inbox.receive().await;
        inbox.try_deliver(Some(&scope("main")), |text| text == "still here");
        assert!(inbox.can_receive());
    }

    #[tokio::test]
    async fn shutdown_retires_a_reader_waiting_for_ui_acceptance() {
        let (mut inbox, mut paths) = inbox_with_text("one handed-over prompt");
        inbox.set_scope(Some(scope("main")), None);
        inbox.receive().await;
        paths.recv().await.unwrap();

        tokio::time::timeout(Duration::from_secs(1), inbox.shutdown())
            .await
            .expect("shutdown must not wait for UI acceptance");
        assert!(paths.recv().await.is_none());
    }

    #[tokio::test]
    async fn shutdown_finishes_an_inflight_read_without_starting_another() {
        let finish = Arc::new(Notify::new());
        let read_finish = Arc::clone(&finish);
        let (started, mut starts) = mpsc::unbounded_channel();
        let (completed, mut completions) = mpsc::unbounded_channel();
        let inbox = PromptInbox::spawn_with_reader(move |path| {
            let finish = Arc::clone(&read_finish);
            let completed = completed.clone();
            let _ = started.send(path);
            async move {
                finish.notified().await;
                let _ = completed.send(());
                Ok(Vec::new())
            }
        });
        inbox.set_scope(Some(scope("main")), None);
        starts.recv().await.unwrap();
        let shutdown = tokio::spawn(inbox.shutdown());
        tokio::task::yield_now().await;
        assert!(!shutdown.is_finished());
        finish.notify_one();
        completions.recv().await.unwrap();
        shutdown.await.unwrap();
        assert!(starts.recv().await.is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn old_kernel_errors_back_off_without_stopping_the_cli() {
        let (paths, mut paths_rx) = mpsc::unbounded_channel();
        let inbox = PromptInbox::spawn_with_reader(move |path| {
            let _ = paths.send(path);
            async { Err(-8) }
        });
        assert_scope_path(paths_rx.recv().await.unwrap(), "offline");
        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(paths_rx.try_recv().is_err());
        tokio::time::advance(Duration::from_secs(1)).await;
        assert_scope_path(paths_rx.recv().await.unwrap(), "offline");
        inbox.set_scope(Some(scope("new")), None);
        assert_scope_path(paths_rx.recv().await.unwrap(), "new");
        inbox.set_scope(None, None);
        assert_scope_path(paths_rx.recv().await.unwrap(), "offline");
    }
}
