//! Shared Codex API, HTTP, retry, and WebSocket implementations.
//!
//! Keeping these implementations in one crate removes crate-to-crate code generation barriers;
//! their module boundaries preserve the distinct HTTP, generic retry, and API interfaces.

pub mod api;
pub mod client;
pub mod http;
pub mod websocket;
