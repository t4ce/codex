//! TRUEOS foreground services are owned by the native VM lifecycle.
//!
//! Stop/Kill acts on the realm, without sending a Unix signal into it. A
//! service may still finish through its own control channel or transport EOF.
//! This wait registers no signal handler and never reports a Unix interrupt.

/// Remain alive while the host owns this VM's lifetime. The host can stop the
/// VM without this future producing a successful signal notification. Select
/// service-specific cancellation or transport EOF alongside this wait.
pub async fn wait_for_vm_stop() -> ! {
    core::future::pending().await
}
