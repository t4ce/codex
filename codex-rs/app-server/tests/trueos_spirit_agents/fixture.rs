//! Host fixtures for the production native observer. No live OS actions are performed.
#![allow(dead_code, unused_imports)]

extern crate alloc;
extern crate self as codex_app_server_protocol;
extern crate self as codex_core;
extern crate self as codex_protocol;
extern crate self as trueos;
extern crate self as trueos_v;

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::RwLock;
use tokio::sync::broadcast;
use tokio::sync::watch;

#[path = "@CODEX_RS@/core/src/thread_manager/trueos_spirit_agents.rs"]
mod core_projection;
#[path = "@CODEX_RS@/app-server/src/trueos_spirit_agents.rs"]
mod publisher;
#[path = "@REPOSITORIES@/TRUEOS/crates/trueos-v/src/spirit_agents.rs"]
pub mod spirit_agents;
#[path = "watch.rs"]
mod thread_status;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ThreadId(u64);
impl fmt::Display for ThreadId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "agent-{}", self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThreadActiveFlag {
    WaitingOnApproval,
    WaitingOnUserInput,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThreadStatus {
    NotLoaded,
    Idle,
    SystemError,
    Active { active_flags: Vec<ThreadActiveFlag> },
}
#[derive(Clone)]
pub struct ThreadStatusChangedNotification {
    thread_id: String,
    status: ThreadStatus,
}
pub enum ServerNotification {
    ThreadStatusChanged(ThreadStatusChangedNotification),
}
pub mod outgoing_message {
    pub struct OutgoingEnvelope;
    pub struct OutgoingMessage;
    pub struct OutgoingMessageSender;
    impl OutgoingMessageSender {
        pub async fn send_server_notification(&self, _: crate::ServerNotification) {}
    }
}

#[derive(Clone, Debug)]
pub struct AgentPath(String);
impl AgentPath {
    fn as_str(&self) -> &str {
        &self.0
    }
}
impl fmt::Display for AgentPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
pub mod protocol {
    use super::*;
    #[derive(Clone)]
    pub enum SubAgentSource {
        ThreadSpawn {
            parent_thread_id: ThreadId,
            agent_path: Option<AgentPath>,
            agent_nickname: Option<String>,
            agent_role: Option<String>,
        },
        Review,
    }
    #[derive(Clone)]
    pub enum SessionSource {
        Cli,
        Internal,
        SubAgent(SubAgentSource),
    }
    impl SessionSource {
        pub fn is_internal(&self) -> bool {
            matches!(self, Self::Internal)
        }
        pub fn parent_thread_id(&self) -> Option<ThreadId> {
            match self {
                Self::SubAgent(SubAgentSource::ThreadSpawn {
                    parent_thread_id, ..
                }) => Some(*parent_thread_id),
                _ => None,
            }
        }
    }
}
pub mod codex_thread {
    use super::*;
    #[derive(Clone)]
    pub struct ThreadConfigSnapshot {
        pub ephemeral: bool,
        pub parent_thread_id: Option<ThreadId>,
        pub session_source: protocol::SessionSource,
    }
}
pub mod agent {
    #[derive(Clone, Debug)]
    pub enum AgentStatus {
        PendingInit,
        Running,
        Interrupted,
        Completed(Option<String>),
        Errored(String),
        Shutdown,
        NotFound,
    }
    pub mod types {
        use super::super::*;
        #[derive(Clone, Default)]
        pub struct AgentMetadata {
            pub agent_id: Option<ThreadId>,
            pub agent_path: Option<AgentPath>,
            pub agent_nickname: Option<String>,
            pub agent_role: Option<String>,
        }
    }
}
pub struct FakeThread {
    session: FakeSession,
    config: codex_thread::ThreadConfigSnapshot,
    status: RwLock<agent::AgentStatus>,
}
pub struct FakeSession {
    id: ThreadId,
    services: FakeServices,
}
impl FakeSession {
    fn session_id(&self) -> ThreadId {
        self.id
    }
}
pub struct FakeServices {
    local_agent_runtime: FakeRuntime,
}
pub struct FakeRuntime(Arc<Mutex<Vec<agent::types::AgentMetadata>>>);
impl FakeRuntime {
    fn control(&self, _: ThreadId) -> Controller {
        Controller(Arc::clone(&self.0))
    }
}
impl FakeThread {
    async fn config_snapshot(&self) -> codex_thread::ThreadConfigSnapshot {
        self.config.clone()
    }
    async fn agent_status(&self) -> agent::AgentStatus {
        self.status.read().unwrap().clone()
    }
}
#[derive(Clone)]
pub struct Controller(Arc<Mutex<Vec<agent::types::AgentMetadata>>>);
impl Controller {
    fn shares_spirit_tree(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    fn registered_spirit_agents(&self) -> Vec<agent::types::AgentMetadata> {
        self.0.lock().unwrap().clone()
    }
    fn get_agent_metadata(&self, id: ThreadId) -> Option<agent::types::AgentMetadata> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .find(|member| member.agent_id == Some(id))
            .cloned()
    }
}
pub struct ThreadManager {
    loaded: RwLock<HashMap<ThreadId, Arc<FakeThread>>>,
    metadata: Arc<Mutex<Vec<agent::types::AgentMetadata>>>,
    created: broadcast::Sender<ThreadId>,
}
impl ThreadManager {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            loaded: RwLock::default(),
            metadata: Arc::default(),
            created: broadcast::channel(64).0,
        })
    }
    fn agent_control(&self) -> Controller {
        // The production manager constructs a fresh tree here. Existing
        // metadata belongs to the loaded session's runtime, not this handle.
        Controller(Arc::default())
    }
    async fn list_thread_ids(&self) -> Vec<ThreadId> {
        self.loaded.read().unwrap().keys().copied().collect()
    }
    async fn get_thread(&self, id: ThreadId) -> Result<Arc<FakeThread>, ()> {
        self.loaded.read().unwrap().get(&id).cloned().ok_or(())
    }
    fn subscribe_thread_created(&self) -> broadcast::Receiver<ThreadId> {
        self.created.subscribe()
    }
    fn insert(&self, id: u64, parent: Option<u64>, path: &str, status: agent::AgentStatus) {
        let source = match parent {
            Some(parent) => {
                protocol::SessionSource::SubAgent(protocol::SubAgentSource::ThreadSpawn {
                    parent_thread_id: ThreadId(parent),
                    agent_path: Some(AgentPath(path.into())),
                    agent_nickname: Some("Scout".into()),
                    agent_role: Some("explorer".into()),
                })
            }
            None => protocol::SessionSource::Cli,
        };
        self.loaded.write().unwrap().insert(
            ThreadId(id),
            Arc::new(FakeThread {
                session: FakeSession {
                    id: ThreadId(id),
                    services: FakeServices {
                        local_agent_runtime: FakeRuntime(Arc::clone(&self.metadata)),
                    },
                },
                config: codex_thread::ThreadConfigSnapshot {
                    ephemeral: false,
                    parent_thread_id: parent.map(ThreadId),
                    session_source: source,
                },
                status: RwLock::new(status),
            }),
        );
        let mut metadata = self.metadata.lock().unwrap();
        metadata.retain(|entry| entry.agent_id != Some(ThreadId(id)));
        metadata.push(agent::types::AgentMetadata {
            agent_id: Some(ThreadId(id)),
            agent_path: Some(AgentPath(path.into())),
            agent_nickname: Some("Scout".into()),
            agent_role: Some("explorer".into()),
        });
        drop(metadata);
        let _ = self.created.send(ThreadId(id));
    }
    fn status(&self, id: u64, status: agent::AgentStatus) {
        *self.loaded.read().unwrap()[&ThreadId(id)]
            .status
            .write()
            .unwrap() = status;
    }
}

pub mod spirit {
    use super::*;
    #[derive(Debug)]
    pub struct Error(pub i32);
    pub struct AbiState {
        pub accepted: Vec<spirit_agents::AgentSnapshot>,
        pub failures: usize,
        pub attempts: usize,
    }
    pub fn abi() -> &'static Mutex<AbiState> {
        static ABI: OnceLock<Mutex<AbiState>> = OnceLock::new();
        ABI.get_or_init(|| {
            Mutex::new(AbiState {
                accepted: Vec::new(),
                failures: 0,
                attempts: 0,
            })
        })
    }
    pub fn publish_agent_snapshot(snapshot: &spirit_agents::AgentSnapshot) -> Result<(), Error> {
        let mut state = abi().lock().unwrap();
        state.attempts += 1;
        let bytes = spirit_agents::encode(snapshot).map_err(|_| {
            state.failures += 1;
            Error(-4)
        })?;
        let decoded = spirit_agents::decode(&bytes).unwrap();
        state.accepted.push(decoded);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spirit_agents::AgentLifecycle as Life;
    use spirit_agents::AgentRuntime as Runtime;
    use spirit_agents::AgentSnapshot;
    use std::time::Duration;

    async fn serial() -> tokio::sync::MutexGuard<'static, ()> {
        static SERIAL: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
        let guard = SERIAL
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await;
        *spirit::abi().lock().unwrap() = spirit::AbiState {
            accepted: Vec::new(),
            failures: 0,
            attempts: 0,
        };
        guard
    }
    async fn until(predicate: impl Fn(&AgentSnapshot) -> bool) -> AgentSnapshot {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Some(snapshot) = spirit::abi()
                    .lock()
                    .unwrap()
                    .accepted
                    .last()
                    .filter(|s| predicate(s))
                    .cloned()
                {
                    return snapshot;
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("native projection did not observe the source change")
    }
    fn child(snapshot: &AgentSnapshot) -> &spirit_agents::AgentEntry {
        snapshot
            .agents
            .iter()
            .find(|entry| entry.id == "agent-2")
            .unwrap()
    }

    #[tokio::test]
    async fn automatic_snapshot_tracks_exact_lifecycle_and_parent_independent_of_any_tui() {
        let _serial = serial().await;
        let manager = ThreadManager::new();
        let watch = thread_status::ThreadWatchManager::new();
        let publisher = publisher::SpiritAgentPublisher::spawn(manager.clone(), watch.clone());
        let initial = until(|s| s.connected && s.agents.is_empty()).await;
        assert!(initial.complete && initial.epoch > 0 && initial.revision > 0);
        manager.insert(1, None, "/root", agent::AgentStatus::Running);
        manager.insert(2, Some(1), "/root/scout", agent::AgentStatus::PendingInit);
        let initialized =
            until(|s| s.agents.len() == 1 && child(s).lifecycle == Some(Life::PendingInit)).await;
        assert_eq!(child(&initialized).parent_id.as_deref(), Some("agent-1"));
        assert_eq!(child(&initialized).name.as_deref(), Some("/root/scout"));
        assert_eq!(child(&initialized).nickname.as_deref(), Some("Scout"));
        assert_eq!(child(&initialized).role.as_deref(), Some("explorer"));
        for (source, expected) in [
            (agent::AgentStatus::Running, Life::Running),
            (
                agent::AgentStatus::Completed(Some("real result".into())),
                Life::Completed,
            ),
            (agent::AgentStatus::Errored("failure".into()), Life::Errored),
            (agent::AgentStatus::Interrupted, Life::Interrupted),
            (agent::AgentStatus::Shutdown, Life::Shutdown),
        ] {
            manager.status(2, source);
            watch.note_turn_completed("agent-2", false).await;
            until(|s| child(s).lifecycle == Some(expected)).await;
        }
        publisher.shutdown();
        until(|s| !s.connected && s.agents.is_empty()).await;
    }

    #[tokio::test]
    async fn nickname_uses_canonical_registry_then_session_source_without_changing_task_path() {
        let manager = ThreadManager::new();
        manager.insert(1, None, "/root", agent::AgentStatus::Running);
        manager.insert(2, Some(1), "/root/scout", agent::AgentStatus::Running);
        {
            let mut registry = manager.metadata.lock().unwrap();
            registry
                .iter_mut()
                .find(|entry| entry.agent_id == Some(ThreadId(2)))
                .unwrap()
                .agent_nickname = Some("Lovelace".into());
        }
        let entries = manager.spirit_agent_entries().await;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "agent-2");
        assert_eq!(entries[0].name.as_deref(), Some("/root/scout"));
        assert_eq!(entries[0].nickname.as_deref(), Some("Lovelace"));
        {
            let mut registry = manager.metadata.lock().unwrap();
            let entry = registry
                .iter_mut()
                .find(|entry| entry.agent_id == Some(ThreadId(2)))
                .unwrap();
            entry.agent_nickname = None;
            entry.agent_path = None;
        }
        let entries = manager.spirit_agent_entries().await;
        assert_eq!(entries[0].name.as_deref(), Some("/root/scout"));
        assert_eq!(entries[0].nickname.as_deref(), Some("Scout"));
        {
            let mut loaded = manager.loaded.write().unwrap();
            let thread = Arc::get_mut(loaded.get_mut(&ThreadId(2)).unwrap()).unwrap();
            let protocol::SessionSource::SubAgent(protocol::SubAgentSource::ThreadSpawn {
                agent_nickname,
                ..
            }) = &mut thread.config.session_source
            else {
                panic!("expected spawned child");
            };
            *agent_nickname = None;
        }
        let entries = manager.spirit_agent_entries().await;
        assert_eq!(entries[0].name.as_deref(), Some("/root/scout"));
        assert_eq!(
            entries[0].nickname, None,
            "a missing nickname must not be invented"
        );
    }

    #[tokio::test]
    async fn unloaded_agents_keep_registry_nickname_without_inventing_runtime_or_name() {
        let manager = ThreadManager::new();
        manager.insert(1, None, "/root", agent::AgentStatus::Running);
        manager.insert(2, Some(1), "/root/scout", agent::AgentStatus::Running);
        manager.loaded.write().unwrap().remove(&ThreadId(2));
        let entries = manager.spirit_agent_entries().await;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name.as_deref(), Some("/root/scout"));
        assert_eq!(entries[0].nickname.as_deref(), Some("Scout"));
        assert!(!entries[0].loaded);
        assert_eq!(entries[0].lifecycle, None);
        assert_eq!(entries[0].runtime, Some(Runtime::NotLoaded));
        {
            let mut registry = manager.metadata.lock().unwrap();
            registry
                .iter_mut()
                .find(|entry| entry.agent_id == Some(ThreadId(2)))
                .unwrap()
                .agent_nickname = None;
        }
        let entries = manager.spirit_agent_entries().await;
        assert_eq!(entries[0].nickname, None);
        assert_eq!(entries[0].name.as_deref(), Some("/root/scout"));
    }

    #[tokio::test]
    async fn approval_and_input_guards_preserve_lifecycle_and_release_attention() {
        let _serial = serial().await;
        let manager = ThreadManager::new();
        manager.insert(1, None, "/root", agent::AgentStatus::Running);
        manager.insert(2, Some(1), "/root/scout", agent::AgentStatus::Running);
        let watch = thread_status::ThreadWatchManager::new();
        watch.note_turn_started("agent-2").await;
        let publisher = publisher::SpiritAgentPublisher::spawn(manager.clone(), watch.clone());
        until(|s| s.agents.len() == 1).await;
        let approval = watch.note_permission_requested("agent-2").await;
        let snapshot = until(|s| child(s).waiting_approval).await;
        assert_eq!(child(&snapshot).lifecycle, Some(Life::Running));
        assert_eq!(child(&snapshot).runtime, Some(Runtime::Active));
        let question = watch.note_user_input_requested("agent-2").await;
        until(|s| child(s).waiting_input && child(s).waiting_approval).await;
        drop(approval);
        until(|s| child(s).waiting_input && !child(s).waiting_approval).await;
        drop(question);
        until(|s| {
            !child(s).waiting_input
                && !child(s).waiting_approval
                && child(s).lifecycle == Some(Life::Running)
        })
        .await;
        publisher.shutdown();
    }

    #[tokio::test]
    async fn known_unloaded_child_stays_unknown_and_reload_uses_actual_runtime() {
        let _serial = serial().await;
        let manager = ThreadManager::new();
        manager.insert(1, None, "/root", agent::AgentStatus::Running);
        manager.insert(
            2,
            Some(1),
            "/root/scout",
            agent::AgentStatus::Completed(None),
        );
        let watch = thread_status::ThreadWatchManager::new();
        let publisher = publisher::SpiritAgentPublisher::spawn(manager.clone(), watch.clone());
        until(|s| s.agents.len() == 1 && child(s).loaded).await;
        manager.loaded.write().unwrap().remove(&ThreadId(2));
        watch.note_thread_shutdown("agent-2").await;
        let unloaded = until(|s| !child(s).loaded).await;
        assert_eq!(child(&unloaded).lifecycle, None);
        assert_eq!(child(&unloaded).runtime, Some(Runtime::NotLoaded));
        assert_eq!(child(&unloaded).parent_id.as_deref(), Some("agent-1"));
        assert_eq!(child(&unloaded).nickname.as_deref(), Some("Scout"));
        manager.insert(2, Some(1), "/root/scout", agent::AgentStatus::Running);
        watch.note_turn_started("agent-2").await;
        until(|s| child(s).loaded && child(s).lifecycle == Some(Life::Running)).await;
        manager.loaded.write().unwrap().remove(&ThreadId(2));
        manager
            .metadata
            .lock()
            .unwrap()
            .retain(|member| member.agent_id != Some(ThreadId(2)));
        watch.remove_thread("agent-2").await;
        until(|s| s.agents.is_empty()).await;
        publisher.shutdown();
    }

    #[tokio::test]
    async fn over_budget_is_atomic_and_shutdown_cannot_be_revived_by_source_changes() {
        let _serial = serial().await;
        let manager = ThreadManager::new();
        manager.insert(1, None, "/root", agent::AgentStatus::Running);
        manager.insert(2, Some(1), "/root/scout", agent::AgentStatus::Running);
        let watch = thread_status::ThreadWatchManager::new();
        let publisher = publisher::SpiritAgentPublisher::spawn(manager.clone(), watch.clone());
        let accepted = until(|s| s.agents.len() == 1).await;
        for id in 3..=34 {
            manager.insert(
                id,
                Some(1),
                &format!("/root/worker-{id}"),
                agent::AgentStatus::Running,
            );
        }
        watch.note_turn_started("agent-2").await;
        tokio::time::timeout(Duration::from_secs(3), async {
            while spirit::abi().lock().unwrap().failures == 0 {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            spirit::abi()
                .lock()
                .unwrap()
                .accepted
                .last()
                .unwrap()
                .agents,
            accepted.agents
        );
        publisher.shutdown();
        let stopped = until(|s| !s.connected).await;
        watch.note_turn_started("agent-2").await;
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(
            spirit::abi().lock().unwrap().accepted.last().unwrap(),
            &stopped
        );
        drop(publisher);
        let next = publisher::SpiritAgentPublisher::spawn(ThreadManager::new(), watch);
        let fresh = until(|s| s.connected).await;
        assert!(fresh.epoch > stopped.epoch);
        assert_eq!(fresh.revision, 1);
        next.shutdown();
    }
}
