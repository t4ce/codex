//! Read-only projection of existing agent runtimes and registry identities for native Spirit.
//! This does not register, restore, start, interrupt, or acknowledge any agent.

use super::ThreadManager;
use crate::agent::AgentStatus;
use crate::agent::types::AgentMetadata;
use crate::codex_thread::ThreadConfigSnapshot;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::SubAgentSource;
use std::collections::HashMap;
use std::collections::HashSet;
use trueos_v::spirit_agents::AgentEntry;
use trueos_v::spirit_agents::AgentLifecycle;
use trueos_v::spirit_agents::AgentRuntime;

impl ThreadManager {
    /// Snapshot actual loaded agents and known unloaded children without interpreting UI state.
    /// Runtime attention flags are filled by the app server's existing thread watch manager.
    pub async fn spirit_agent_entries(&self) -> Vec<AgentEntry> {
        let controller = self.agent_control();
        let registered = controller.registered_spirit_agents();
        let identities: HashMap<_, _> = registered
            .iter()
            .filter_map(|metadata| {
                Some((
                    metadata.agent_path.as_ref()?.to_string(),
                    metadata.agent_id?,
                ))
            })
            .collect();
        let mut seen = HashSet::new();
        let mut entries = Vec::new();
        for thread_id in self.list_thread_ids().await {
            let Ok(thread) = self.get_thread(thread_id).await else {
                continue;
            };
            let config = thread.config_snapshot().await;
            seen.insert(thread_id);
            if !is_presented_agent(&config) {
                continue;
            }
            let metadata = controller.get_agent_metadata(thread_id).unwrap_or_default();
            let (source_path, source_name, source_role) = match &config.session_source {
                SessionSource::SubAgent(SubAgentSource::ThreadSpawn {
                    agent_path,
                    agent_nickname,
                    agent_role,
                    ..
                }) => (
                    agent_path.as_ref().map(ToString::to_string),
                    agent_nickname.clone(),
                    agent_role.clone(),
                ),
                _ => (None, None, None),
            };
            entries.push(AgentEntry {
                id: thread_id.to_string(),
                parent_id: config
                    .parent_thread_id
                    .or_else(|| config.session_source.parent_thread_id())
                    .map(|id| id.to_string()),
                name: metadata
                    .agent_path
                    .as_ref()
                    .map(ToString::to_string)
                    .or(source_path)
                    .or(metadata.agent_nickname)
                    .or(source_name),
                role: metadata.agent_role.or(source_role),
                lifecycle: Some(lifecycle(&thread.agent_status().await)),
                runtime: None,
                waiting_approval: false,
                waiting_input: false,
                loaded: true,
            });
        }
        for metadata in registered {
            let Some(thread_id) = metadata.agent_id else {
                continue;
            };
            if seen.insert(thread_id)
                && let Some(parent_id) = metadata
                    .agent_path
                    .as_ref()
                    .and_then(|path| path.as_str().rsplit_once('/'))
                    .and_then(|(parent, _)| identities.get(parent))
            {
                entries.push(unloaded_entry(
                    thread_id.to_string(),
                    parent_id.to_string(),
                    metadata,
                ));
            }
        }
        entries.sort_by(|left, right| left.id.cmp(&right.id));
        entries
    }
}

fn is_presented_agent(config: &ThreadConfigSnapshot) -> bool {
    if config.ephemeral || config.session_source.is_internal() {
        return false;
    }
    match &config.session_source {
        SessionSource::SubAgent(SubAgentSource::ThreadSpawn { .. }) => config
            .parent_thread_id
            .or_else(|| config.session_source.parent_thread_id())
            .is_some(),
        _ => false,
    }
}

fn lifecycle(status: &AgentStatus) -> AgentLifecycle {
    match status {
        AgentStatus::PendingInit => AgentLifecycle::PendingInit,
        AgentStatus::Running => AgentLifecycle::Running,
        AgentStatus::Interrupted => AgentLifecycle::Interrupted,
        AgentStatus::Completed(_) => AgentLifecycle::Completed,
        AgentStatus::Errored(_) => AgentLifecycle::Errored,
        AgentStatus::Shutdown => AgentLifecycle::Shutdown,
        AgentStatus::NotFound => AgentLifecycle::NotFound,
    }
}

fn unloaded_entry(id: String, parent_id: String, metadata: AgentMetadata) -> AgentEntry {
    AgentEntry {
        id,
        parent_id: Some(parent_id),
        name: metadata
            .agent_path
            .as_ref()
            .map(ToString::to_string)
            .or(metadata.agent_nickname),
        role: metadata.agent_role,
        lifecycle: None,
        runtime: Some(AgentRuntime::NotLoaded),
        waiting_approval: false,
        waiting_input: false,
        loaded: false,
    }
}
