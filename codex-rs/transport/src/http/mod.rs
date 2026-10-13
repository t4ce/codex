mod chatgpt_cloudflare_cookies;
mod chatgpt_hosts;
mod client;
mod client_builder;
mod client_tls;
mod custom_ca;
mod error;
mod network_policy;
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

#[cfg(windows)]
mod windows_tls;

#[cfg(windows)]
pub use crate::http::windows_tls::build_windows_platform_tls_config;
