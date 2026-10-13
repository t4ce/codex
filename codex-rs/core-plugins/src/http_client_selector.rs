use codex_transport::http::OutboundProxyPolicy;
use codex_transport::http::RequestBuilder;
use codex_transport::http::RouteAwareClientPool;
use http::Method;
use std::fmt::Debug;

/// Builds requests whose URL is also used to resolve their outbound route.
///
/// Implementations must keep route selection coupled to the request URL. Returning a transport
/// client would let callers send a different URL than the one used for route selection.
pub(crate) trait HttpClientSelector: Debug + Send + Sync {
    fn request(&self, method: Method, url: &str) -> RequestBuilder;
    fn outbound_proxy_policy(&self) -> OutboundProxyPolicy;
}

impl HttpClientSelector for RouteAwareClientPool {
    fn request(&self, method: Method, url: &str) -> RequestBuilder {
        RouteAwareClientPool::request(self, method, url)
    }

    fn outbound_proxy_policy(&self) -> OutboundProxyPolicy {
        RouteAwareClientPool::outbound_proxy_policy(self)
    }
}
