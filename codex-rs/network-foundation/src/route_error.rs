use crate::BuildRouteAwareHttpClientError;
use crate::NetworkPolicyDenied;
use crate::RouteFailureClass;
use http::StatusCode;
use std::io;

/// Error returned when selecting a route or constructing its pooled HTTP client.
#[derive(Debug, thiserror::Error)]
pub enum RouteAwareClientPoolError {
    #[error("failed to resolve the outbound proxy route: {0}")]
    Resolve(#[source] io::Error),
    #[error(transparent)]
    Build(#[from] BuildRouteAwareHttpClientError),
    #[error("HTTP transport construction task failed: {0}")]
    BuildTask(#[source] tokio::task::JoinError),
}

/// Error returned while building, routing, or sending a route-aware request.
#[derive(Debug, thiserror::Error)]
pub enum RouteAwareRequestError {
    #[error(transparent)]
    Policy(#[from] NetworkPolicyDenied),
    #[error(transparent)]
    Request(#[from] reqwest::Error),
    #[error(transparent)]
    Route(#[from] RouteAwareClientPoolError),
    #[error("failed to build route-aware request: {0}")]
    Build(String),
    #[error("redirect target uses unsupported URL scheme: {0}")]
    UnsupportedRedirectScheme(String),
    #[error("too many redirects")]
    TooManyRedirects,
    #[error("route-aware request timed out")]
    Timeout,
}

impl RouteAwareRequestError {
    /// Classifies transport, proxy, and certificate failures without exposing request details.
    pub fn failure_class(&self) -> Option<RouteFailureClass> {
        if self.is_timeout() {
            return Some(RouteFailureClass::ConnectTimeout);
        }
        if self.status() == Some(StatusCode::PROXY_AUTHENTICATION_REQUIRED) {
            return Some(RouteFailureClass::ProxyAuthenticationRequired);
        }
        if let Self::Route(RouteAwareClientPoolError::Resolve(error)) = self
            && let Some(source) = error.get_ref()
            && source.is::<rustls::Error>()
        {
            return Some(RouteFailureClass::TlsError);
        }

        let mut source: Option<&(dyn std::error::Error + 'static)> = Some(self);
        while let Some(error) = source {
            if error.downcast_ref::<rustls::Error>().is_some() {
                return Some(RouteFailureClass::TlsError);
            }
            #[cfg(not(target_os = "trueos"))]
            if error.downcast_ref::<native_tls::Error>().is_some() {
                return Some(RouteFailureClass::TlsError);
            }
            if error.to_string() == "tunnel error: proxy authorization required" {
                return Some(RouteFailureClass::ProxyAuthenticationRequired);
            }
            source = error
                .downcast_ref::<io::Error>()
                .and_then(|error| error.get_ref())
                .map(|inner| inner as &(dyn std::error::Error + 'static))
                .or_else(|| error.source());
        }

        match self {
            Self::Route(RouteAwareClientPoolError::Build(
                BuildRouteAwareHttpClientError::CustomCa(_)
                | BuildRouteAwareHttpClientError::ExplicitTls(_),
            )) => Some(RouteFailureClass::TlsError),
            Self::Route(RouteAwareClientPoolError::Build(
                BuildRouteAwareHttpClientError::InvalidProxyConfig { .. },
            )) => Some(RouteFailureClass::InvalidProxyConfig),
            Self::Route(RouteAwareClientPoolError::Resolve(_)) => {
                Some(RouteFailureClass::ProxyResolutionUnavailable)
            }
            Self::Route(RouteAwareClientPoolError::BuildTask(_))
            | Self::Request(_)
            | Self::Policy(_)
            | Self::Build(_)
            | Self::UnsupportedRedirectScheme(_)
            | Self::TooManyRedirects
            | Self::Timeout => None,
        }
    }

    pub fn status(&self) -> Option<StatusCode> {
        match self {
            Self::Request(error) => error.status(),
            Self::Route(_)
            | Self::Policy(_)
            | Self::Build(_)
            | Self::UnsupportedRedirectScheme(_)
            | Self::TooManyRedirects
            | Self::Timeout => None,
        }
    }

    pub fn is_timeout(&self) -> bool {
        matches!(self, Self::Timeout) || matches!(self, Self::Request(error) if error.is_timeout())
    }

    pub fn is_connect(&self) -> bool {
        matches!(self, Self::Request(error) if error.is_connect())
    }

    pub fn is_builder(&self) -> bool {
        matches!(self, Self::Request(error) if error.is_builder())
    }

    pub fn is_body(&self) -> bool {
        matches!(self, Self::Request(error) if error.is_body())
    }

    pub fn is_request(&self) -> bool {
        matches!(self, Self::Request(error) if error.is_request())
    }

    pub fn url_mut(&mut self) -> Option<&mut reqwest::Url> {
        match self {
            Self::Request(error) => error.url_mut(),
            _ => None,
        }
    }

    /// Removes a request URL from the underlying transport error before it is logged or returned.
    ///
    /// Use this for requests whose URL can contain credentials, such as signed blob uploads.
    pub fn without_url(self) -> Self {
        match self {
            Self::Request(error) => Self::Request(error.without_url()),
            other => other,
        }
    }
}
