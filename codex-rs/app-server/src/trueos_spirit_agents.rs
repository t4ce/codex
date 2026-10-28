//! Native display projection of authoritative agent and app-server attention snapshots.
//! This observer does not operate agents, consume their event readers, or interpret a transcript.

use crate::thread_status::ThreadWatchManager;
use codex_app_server_protocol::ThreadActiveFlag;
use codex_app_server_protocol::ThreadStatus;
use codex_core::ThreadManager;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::task::JoinHandle;
use trueos_v::spirit_agents::AgentEntry;
use trueos_v::spirit_agents::AgentRuntime;
use trueos_v::spirit_agents::AgentSnapshot;

const RESYNC_INTERVAL: Duration = Duration::from_secs(2);
static NEXT_EPOCH: AtomicU64 = AtomicU64::new(1);

/// Owned by the embedded server, independently of the TUI's selected conversation or `/pet`.
pub(crate) struct SpiritAgentPublisher {
    task: JoinHandle<()>,
    publication: Arc<Mutex<Publication>>,
}

impl SpiritAgentPublisher {
    pub(crate) fn spawn(manager: Arc<ThreadManager>, thread_watch: ThreadWatchManager) -> Self {
        // Subscribe before the first snapshot so creation/attention changes cannot be lost.
        let mut created = manager.subscribe_thread_created();
        let mut changes = thread_watch.subscribe_spirit_changes();
        let publication = Arc::new(Mutex::new(Publication::new(
            NEXT_EPOCH.fetch_add(1, Ordering::Relaxed),
        )));
        let worker_publication = Arc::clone(&publication);
        let task = tokio::spawn(async move {
            // A bounded source resync covers unload/reload races and coalesced watch updates.
            // It rereads canonical membership; it does not advance agent state or animations.
            let mut resync = tokio::time::interval_at(
                tokio::time::Instant::now() + RESYNC_INTERVAL,
                RESYNC_INTERVAL,
            );
            loop {
                changes.borrow_and_update();
                let mut entries = manager.spirit_agent_entries().await;
                let statuses = thread_watch
                    .loaded_statuses_for_threads(entries.iter().map(|entry| entry.id.clone()))
                    .await;
                apply_attention(&mut entries, &statuses);
                worker_publication
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .publish(entries);
                tokio::select! {
                    _ = changes.changed() => {},
                    _ = created.recv() => {},
                    _ = resync.tick() => {},
                }
            }
        });
        Self { task, publication }
    }

    pub(crate) fn shutdown(&self) {
        self.task.abort();
        self.publication
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .shutdown();
    }
}

impl Drop for SpiritAgentPublisher {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Only transport state lives here: no independent agent lifecycle or attention policy.
struct Publication {
    epoch: u64,
    revision: u64,
    last_accepted: Option<Vec<AgentEntry>>,
    last_error: Option<i32>,
    closed: bool,
}

impl Publication {
    fn new(epoch: u64) -> Self {
        Self {
            epoch,
            revision: 0,
            last_accepted: None,
            last_error: None,
            closed: false,
        }
    }

    fn snapshot(&mut self, entries: Vec<AgentEntry>, connected: bool) -> AgentSnapshot {
        self.revision = self.revision.wrapping_add(1).max(1);
        AgentSnapshot {
            version: 1,
            epoch: self.epoch,
            revision: self.revision,
            complete: true,
            connected,
            agents: entries,
        }
    }

    fn publish(&mut self, entries: Vec<AgentEntry>) {
        if self.closed || self.last_accepted.as_ref() == Some(&entries) {
            return;
        }
        let snapshot = self.snapshot(entries, true);
        match trueos::spirit::publish_agent_snapshot(&snapshot) {
            Ok(()) => {
                self.last_accepted = Some(snapshot.agents);
                self.last_error = None;
            }
            Err(error) => {
                // Preserve the last accepted complete projection; never silently truncate it.
                if self.last_error != Some(error.0) {
                    tracing::warn!(
                        code = error.0,
                        agents = snapshot.agents.len(),
                        "native Spirit rejected agent snapshot; retaining previous projection"
                    );
                    self.last_error = Some(error.0);
                }
            }
        }
    }

    fn shutdown(&mut self) {
        if self.closed {
            return;
        }
        // Guard against a worker that was already preparing a snapshot when it was aborted.
        self.closed = true;
        let snapshot = self.snapshot(Vec::new(), false);
        if let Err(error) = trueos::spirit::publish_agent_snapshot(&snapshot) {
            tracing::debug!(
                code = error.0,
                "could not release native Spirit agent projection"
            );
        }
        self.last_accepted = None;
    }
}

fn apply_attention(entries: &mut [AgentEntry], statuses: &HashMap<String, ThreadStatus>) {
    for entry in entries {
        // A known unloaded identity must not inherit a stale loaded runtime's attention flags.
        if !entry.loaded {
            entry.runtime = Some(AgentRuntime::NotLoaded);
            entry.waiting_approval = false;
            entry.waiting_input = false;
            continue;
        }
        let Some(status) = statuses.get(&entry.id) else {
            continue;
        };
        let runtime = match status {
            ThreadStatus::NotLoaded => AgentRuntime::NotLoaded,
            ThreadStatus::Idle => AgentRuntime::Idle,
            ThreadStatus::SystemError => AgentRuntime::SystemError,
            ThreadStatus::Active { active_flags } => {
                entry.waiting_approval =
                    active_flags.contains(&ThreadActiveFlag::WaitingOnApproval);
                entry.waiting_input = active_flags.contains(&ThreadActiveFlag::WaitingOnUserInput);
                AgentRuntime::Active
            }
        };
        entry.runtime = Some(runtime);
    }
}
