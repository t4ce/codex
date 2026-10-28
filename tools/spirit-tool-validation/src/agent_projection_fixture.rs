//! Small runtime inputs for the production native agent projection.
//! These tests execute its real async snapshot method; no agent or HID is started.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

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
        #[derive(Clone, Default)]
        pub struct AgentMetadata {
            pub agent_id: Option<u64>,
            pub agent_path: Option<String>,
            pub agent_nickname: Option<String>,
            pub agent_role: Option<String>,
        }
    }
}

pub mod protocol {
    #[derive(Clone)]
    pub enum SessionSource {
        Root,
        Internal,
        SubAgent(SubAgentSource),
    }

    impl SessionSource {
        pub fn is_internal(&self) -> bool {
            matches!(self, Self::Internal)
        }

        pub fn parent_thread_id(&self) -> Option<u64> {
            match self {
                Self::SubAgent(SubAgentSource::ThreadSpawn {
                    parent_thread_id, ..
                }) => Some(*parent_thread_id),
                _ => None,
            }
        }
    }

    #[derive(Clone)]
    pub enum SubAgentSource {
        ThreadSpawn {
            parent_thread_id: u64,
            depth: i32,
            agent_path: Option<String>,
            agent_nickname: Option<String>,
            agent_role: Option<String>,
        },
        Other,
    }
}

pub mod codex_thread {
    use super::protocol::SessionSource;

    #[derive(Clone)]
    pub struct ThreadConfigSnapshot {
        pub ephemeral: bool,
        pub parent_thread_id: Option<u64>,
        pub session_source: SessionSource,
    }
}

#[derive(Default)]
struct Registry {
    agents: HashMap<u64, agent::types::AgentMetadata>,
    reads: AtomicUsize,
}

#[derive(Clone)]
pub struct Controller(Arc<Registry>);

impl Controller {
    pub fn registered_spirit_agents(&self) -> Vec<agent::types::AgentMetadata> {
        self.0.reads.fetch_add(1, Ordering::Relaxed);
        self.0.agents.values().cloned().collect()
    }

    pub fn get_agent_metadata(&self, id: u64) -> Option<agent::types::AgentMetadata> {
        self.0.agents.get(&id).cloned()
    }

    pub fn shares_spirit_tree(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

pub struct Runtime(Controller);
impl Runtime {
    pub fn control(&self, _session_id: u64) -> Controller {
        self.0.clone()
    }
}

pub struct Services {
    pub local_agent_runtime: Runtime,
}

pub struct Session {
    pub services: Services,
    id: u64,
}
impl Session {
    pub fn session_id(&self) -> u64 {
        self.id
    }
}

pub struct Thread {
    pub session: Session,
    config: codex_thread::ThreadConfigSnapshot,
    status: agent::AgentStatus,
}
impl Thread {
    pub async fn config_snapshot(&self) -> codex_thread::ThreadConfigSnapshot {
        self.config.clone()
    }

    pub async fn agent_status(&self) -> agent::AgentStatus {
        self.status.clone()
    }
}

#[derive(Default)]
pub struct ThreadManager {
    threads: BTreeMap<u64, Arc<Thread>>,
}
impl ThreadManager {
    // Match ThreadManager's fresh-tree constructor, so consulting this method
    // instead of a loaded session's runtime loses the fixture's metadata too.
    pub fn agent_control(&self) -> Controller {
        tree([])
    }

    pub async fn list_thread_ids(&self) -> Vec<u64> {
        self.threads.keys().copied().collect()
    }

    pub async fn get_thread(&self, id: u64) -> Result<Arc<Thread>, ()> {
        self.threads.get(&id).cloned().ok_or(())
    }

    fn add(
        &mut self,
        id: u64,
        controller: &Controller,
        config: codex_thread::ThreadConfigSnapshot,
    ) {
        self.threads.insert(
            id,
            Arc::new(Thread {
                session: Session {
                    id,
                    services: Services {
                        local_agent_runtime: Runtime(controller.clone()),
                    },
                },
                config,
                status: agent::AgentStatus::Running,
            }),
        );
    }
}

fn metadata(id: u64, path: &str, nickname: Option<&str>) -> agent::types::AgentMetadata {
    agent::types::AgentMetadata {
        agent_id: Some(id),
        agent_path: Some(path.into()),
        agent_nickname: nickname.map(str::to_owned),
        agent_role: Some("explorer".into()),
    }
}

fn tree(agents: impl IntoIterator<Item = agent::types::AgentMetadata>) -> Controller {
    Controller(Arc::new(Registry {
        agents: agents
            .into_iter()
            .map(|metadata| (metadata.agent_id.unwrap(), metadata))
            .collect(),
        reads: AtomicUsize::new(0),
    }))
}

fn source(parent: u64, path: &str, nickname: Option<&str>) -> codex_thread::ThreadConfigSnapshot {
    codex_thread::ThreadConfigSnapshot {
        ephemeral: false,
        parent_thread_id: Some(parent),
        session_source: protocol::SessionSource::SubAgent(protocol::SubAgentSource::ThreadSpawn {
            parent_thread_id: parent,
            depth: 1,
            agent_path: Some(path.into()),
            agent_nickname: nickname.map(str::to_owned),
            agent_role: Some("explorer".into()),
        }),
    }
}

fn root() -> codex_thread::ThreadConfigSnapshot {
    codex_thread::ThreadConfigSnapshot {
        ephemeral: false,
        parent_thread_id: None,
        session_source: protocol::SessionSource::Root,
    }
}

#[test]
fn projection_reads_nickname_from_the_existing_tree_even_without_source_metadata() {
    let controller = tree([
        metadata(1, "/root", None),
        metadata(2, "/root/workspace_file_check", Some("Atlas")),
    ]);
    let mut manager = ThreadManager::default();
    manager.add(1, &controller, root());
    manager.add(
        2,
        &controller,
        source(1, "/root/workspace_file_check", None),
    );
    let entries = futures::executor::block_on(manager.spirit_agent_entries());
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].nickname.as_deref(), Some("Atlas"));
    assert_eq!(
        entries[0].name.as_deref(),
        Some("/root/workspace_file_check")
    );
    assert_eq!(controller.0.reads.load(Ordering::Relaxed), 1);
}

#[test]
fn projection_preserves_source_nickname_before_registry_registration() {
    let controller = tree([metadata(1, "/root", None)]);
    let mut manager = ThreadManager::default();
    manager.add(1, &controller, root());
    manager.add(2, &controller, source(1, "/root/check", Some("Hypatia")));
    let entries = futures::executor::block_on(manager.spirit_agent_entries());
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].nickname.as_deref(), Some("Hypatia"));
}

#[test]
fn projection_keeps_canonical_nickname_for_unloaded_v2_children() {
    let controller = tree([
        metadata(1, "/root", None),
        metadata(2, "/root/check", Some("Ada")),
        metadata(3, "/root/check/nested", Some("Turing")),
    ]);
    let mut manager = ThreadManager::default();
    manager.add(1, &controller, root());
    let entries = futures::executor::block_on(manager.spirit_agent_entries());
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].nickname.as_deref(), Some("Ada"));
    assert_eq!(entries[0].parent_id.as_deref(), Some("1"));
    assert_eq!(entries[1].nickname.as_deref(), Some("Turing"));
    assert_eq!(entries[1].parent_id.as_deref(), Some("2"));
    assert!(entries.iter().all(|entry| !entry.loaded));
}

#[test]
fn projection_resolves_parent_paths_within_each_tree() {
    let first = tree([
        metadata(1, "/root", None),
        metadata(2, "/root/joker", Some("Plato")),
    ]);
    let second = tree([
        metadata(3, "/root", None),
        metadata(4, "/root/joker", Some("Darwin")),
    ]);
    let mut manager = ThreadManager::default();
    manager.add(1, &first, root());
    manager.add(3, &second, root());
    let entries = futures::executor::block_on(manager.spirit_agent_entries());
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].parent_id.as_deref(), Some("1"));
    assert_eq!(entries[1].parent_id.as_deref(), Some("3"));
    assert_eq!(entries[0].nickname.as_deref(), Some("Plato"));
    assert_eq!(entries[1].nickname.as_deref(), Some("Darwin"));
}

#[test]
fn missing_nickname_is_not_replaced_with_task_path_or_an_invented_name() {
    let controller = tree([metadata(1, "/root", None)]);
    let mut manager = ThreadManager::default();
    manager.add(1, &controller, root());
    manager.add(2, &controller, source(1, "/root/check", None));
    let entries = futures::executor::block_on(manager.spirit_agent_entries());
    assert_eq!(entries[0].name.as_deref(), Some("/root/check"));
    assert_eq!(entries[0].nickname, None);
}
