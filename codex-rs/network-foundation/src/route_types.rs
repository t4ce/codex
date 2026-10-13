use crate::BuildCustomCaTransportError;
use std::fmt;
use std::io;
use thiserror::Error;

/// Coarse semantic bucket for the HTTP or WebSocket client being constructed.
///
/// This is not the selected proxy route or a concrete endpoint. It labels the
/// product path that owns the client so proxy-resolution diagnostics can
/// distinguish auth, API, WebSocket, and miscellaneous traffic without exposing
/// endpoint details.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientRouteClass {
    /// Login, token refresh/revoke, PAT, and agent identity auth traffic.
    Auth,
    /// First-party API traffic that is not part of the auth flow.
    Api,
    /// WebSocket traffic.
    WebSocket,
    /// Call sites without a more specific route class.
    Other,
}

impl fmt::Display for ClientRouteClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Auth => "auth",
            Self::Api => "api",
            Self::WebSocket => "wss",
            Self::Other => "other",
        })
    }
}

/// Coarse failure class for route selection errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteFailureClass {
    ProxyResolutionUnavailable,
    ConnectTimeout,
    ProxyAuthenticationRequired,
    TlsError,
    InvalidProxyConfig,
    UnsupportedProxyScheme,
    ResolverError,
}

impl fmt::Display for RouteFailureClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ProxyResolutionUnavailable => "proxy_resolution_unavailable",
            Self::ConnectTimeout => "connect_timeout",
            Self::ProxyAuthenticationRequired => "proxy_407",
            Self::TlsError => "tls_error",
            Self::InvalidProxyConfig => "invalid_proxy_config",
            Self::UnsupportedProxyScheme => "unsupported_proxy_scheme",
            Self::ResolverError => "resolver_error",
        })
    }
}

#[derive(Debug, Error)]
pub enum BuildRouteAwareHttpClientError {
    #[error(transparent)]
    CustomCa(#[from] BuildCustomCaTransportError),

    #[error("Failed to build HTTP client with explicit TLS configuration: {0}")]
    ExplicitTls(reqwest::Error),

    #[error("Failed to configure outbound proxy selected for {route_class}")]
    InvalidProxyConfig { route_class: ClientRouteClass },
}

impl From<BuildRouteAwareHttpClientError> for io::Error {
    fn from(error: BuildRouteAwareHttpClientError) -> Self {
        match error {
            BuildRouteAwareHttpClientError::CustomCa(error) => error.into(),
            BuildRouteAwareHttpClientError::InvalidProxyConfig { .. }
            | BuildRouteAwareHttpClientError::ExplicitTls(_) => io::Error::other(error),
        }
    }
}
