use codex_exec_server::EnvironmentManager;
use codex_transport::http::HttpClientFactory;
use codex_transport::http::OutboundProxyPolicy;

pub mod relay;

/// Builds a manager without environments using the legacy outbound HTTP policy.
pub fn environment_manager_without_environments() -> EnvironmentManager {
    EnvironmentManager::without_environments(HttpClientFactory::new(
        OutboundProxyPolicy::ReqwestDefault,
    ))
}
