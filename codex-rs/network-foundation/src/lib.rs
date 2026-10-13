//! Network contracts shared by protocol data and transport implementations.
//!
//! This layer owns error identity, retry deadlines, permission revocation, and the TLS provider.
//! It has no dependency on Codex protocol, API, HTTP execution, or proxy runtime packages.

mod custom_ca_error;
mod network_policy;
mod retry_after;
mod route_error;
mod route_types;
mod rustls_provider;

pub use custom_ca_error::BuildCustomCaTransportError;
pub use network_policy::*;
pub use retry_after::RetryAfter;
pub use route_error::RouteAwareClientPoolError;
pub use route_error::RouteAwareRequestError;
pub use route_types::BuildRouteAwareHttpClientError;
pub use route_types::ClientRouteClass;
pub use route_types::RouteFailureClass;
pub use rustls_provider::ensure_rustls_crypto_provider;

pub type HttpError = RouteAwareRequestError;
