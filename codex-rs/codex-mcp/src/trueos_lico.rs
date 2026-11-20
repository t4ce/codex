//! The kernel-authorized, VM-local Lico endpoint.
use std::net::Ipv4Addr;
use std::time::Duration;

use codex_config::McpServerConfig;
use codex_config::McpServerTransportConfig;

pub const SERVER: &str = "lico";
const URL: &str = "http://127.0.0.1:42127/mcp";

pub fn is_builtin_endpoint(config: &McpServerConfig) -> bool {
    config.is_local_environment()
        && matches!(&config.transport, McpServerTransportConfig::StreamableHttp { url, .. } if url == URL)
}

/// Older kernels do not enforce app ownership for this loopback port.
pub(crate) fn kernel_protects_endpoint() -> bool {
    std::env::var_os("TRUEOS_LICO_MCP_ACL").is_some_and(|value| value == "1")
}

pub async fn endpoint_is_listening() -> bool {
    kernel_protects_endpoint()
        && tokio::time::timeout(
            Duration::from_millis(150),
            tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, 42127)),
        )
        .await
        .is_ok_and(|result| result.is_ok())
}
