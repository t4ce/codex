use url::Host;
use url::Url;

mod chatgpt_cloudflare_cookies;
mod chatgpt_hosts;
mod chatgpt_routing_affinity;
mod client;
mod client_builder;
mod client_tls;
mod custom_ca;
mod error;
mod native_roots;
mod network_policy;
mod openai_endpoints;
mod openai_headers;
mod outbound_proxy;
mod request;
mod request_builder;
mod request_draft;
mod response;
mod retry_after;
mod route_aware_client_pool;
mod route_aware_redirect;
mod tls_backend_fallback;
mod transport;

pub use crate::http::chatgpt_cloudflare_cookies::with_chatgpt_cloudflare_cookie_store;
pub use crate::http::chatgpt_hosts::is_allowed_chatgpt_host;
pub use crate::http::chatgpt_routing_affinity::ChatGptRoutingAffinity;
pub use crate::http::client::HttpClient;
pub use crate::http::client::HttpError;
pub use crate::http::client_builder::HttpClientBuilder;
pub use crate::http::client_tls::HttpClientTlsConfig;
pub use crate::http::custom_ca::BuildCustomCaTransportError;
/// Test-only subprocess hook for custom CA coverage.
///
/// This stays public only so the `custom_ca_probe` binary target can reuse the shared helper. It
/// is hidden from normal docs because ordinary callers should use
/// [`build_reqwest_client_with_custom_ca`] instead.
#[doc(hidden)]
pub use crate::http::custom_ca::build_reqwest_client_for_subprocess_tests;
pub use crate::http::custom_ca::build_reqwest_client_with_custom_ca;
pub use crate::http::custom_ca::build_rustls_client_config_with_custom_ca;
pub use crate::http::custom_ca::maybe_build_rustls_client_config_with_custom_ca;
pub use crate::http::error::StreamError;
pub use crate::http::error::TransportError;
pub use crate::http::network_policy::DestinationPolicy;
pub use crate::http::network_policy::NetworkPermit;
pub use crate::http::network_policy::NetworkPolicy;
pub use crate::http::network_policy::NetworkPolicyController;
pub use crate::http::network_policy::NetworkPolicyDenied;
pub use crate::http::network_policy::NetworkPolicyRevision;
pub use crate::http::openai_endpoints::OpenAiDestination;
pub use crate::http::openai_endpoints::classify_openai_api_url;
pub use crate::http::openai_endpoints::is_openai_api_url;
pub use crate::http::openai_headers::CHATGPT_ACCOUNT_ID_HEADER;
pub use crate::http::openai_headers::OPENAI_ORGANIZATION_HEADER;
pub use crate::http::openai_headers::OPENAI_PROJECT_HEADER;
pub use crate::http::outbound_proxy::BuildRouteAwareHttpClientError;
pub use crate::http::outbound_proxy::ClientRouteClass;
pub use crate::http::outbound_proxy::HttpClientFactory;
#[cfg(target_os = "macos")]
pub use crate::http::outbound_proxy::MacosSystemProxyConfiguration;
pub use crate::http::outbound_proxy::OutboundProxyPolicy;
pub use crate::http::outbound_proxy::OutboundProxyRoute;
pub use crate::http::outbound_proxy::RouteFailureClass;
#[doc(hidden)]
pub use crate::http::outbound_proxy::cache_system_proxy_route_for_test;
#[cfg(target_os = "macos")]
pub use crate::http::outbound_proxy::macos_system_proxy_configuration;
pub use crate::http::request::EncodedJsonBody;
pub use crate::http::request::PreparedRequestBody;
pub use crate::http::request::Request;
pub use crate::http::request::RequestBody;
pub use crate::http::request::RequestCompression;
pub use crate::http::request::Response;
pub use crate::http::request_builder::RequestBuilder;
pub use crate::http::response::HttpResponse;
pub use crate::http::retry_after::RetryAfter;
pub use crate::http::route_aware_client_pool::RouteAwareClientPool;
pub use crate::http::route_aware_client_pool::RouteAwareClientPoolError;
pub use crate::http::route_aware_client_pool::RouteAwareRequestError;
pub use crate::http::transport::ByteStream;
pub use crate::http::transport::HttpTransport;
pub use crate::http::transport::ReqwestTransport;
pub use crate::http::transport::StreamResponse;
pub use crate::http::transport::bounded_response_bytes;

#[cfg(windows)]
mod windows_proxy_auth;
#[cfg(windows)]
mod windows_tls;

#[cfg(windows)]
pub use crate::http::windows_proxy_auth::WindowsProxyTls;
#[cfg(windows)]
pub use crate::http::windows_proxy_auth::connect_windows_proxy;
#[cfg(windows)]
pub use crate::http::windows_tls::build_windows_platform_tls_config;

/// Validates HTTPS or loopback HTTP endpoints, rejecting userinfo and fragments.
/// Errors identify the caller-supplied field without including the URL.
pub fn validate_endpoint_url(value: &str, field: &str) -> Result<Url, String> {
    let url = Url::parse(value).map_err(|_| format!("invalid {field} URL"))?;
    let loopback = match url.host() {
        Some(Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(host)) => host.is_loopback(),
        Some(Host::Ipv6(host)) => host.is_loopback(),
        None => false,
    };
    if url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || !(url.scheme() == "https" || (url.scheme() == "http" && loopback))
    {
        return Err(format!(
            "{field} must use HTTPS (or loopback HTTP), without userinfo or a fragment"
        ));
    }
    Ok(url)
}

/// Validates a credential-bearing HTTPS endpoint without including the URL in errors.
pub fn validate_https_endpoint_url(value: &str, field: &str) -> Result<Url, String> {
    validate_endpoint_url(value, field)
        .ok()
        .filter(|url| url.scheme() == "https")
        .ok_or_else(|| format!("{field} URLs must use HTTPS without userinfo or fragments"))
}
