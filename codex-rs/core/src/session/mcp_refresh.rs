use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use tokio::sync::AcquireError;
use tokio::sync::Semaphore;
use tokio::sync::SemaphorePermit;

/// Owns MCP invalidation and the single gate used to publish runtime updates.
pub(super) struct McpRefresh {
    pending: AtomicBool,
    gate: Semaphore,
    #[cfg(target_os = "trueos")]
    lico: tokio::sync::Mutex<Option<(tokio::time::Instant, bool)>>,
}

impl McpRefresh {
    pub(super) fn new() -> Self {
        Self {
            pending: AtomicBool::new(false),
            gate: Semaphore::new(/*permits*/ 1),
            #[cfg(target_os = "trueos")]
            lico: tokio::sync::Mutex::new(None),
        }
    }

    pub(super) fn invalidate(&self) {
        self.pending.store(true, Ordering::Release);
    }

    #[cfg(test)]
    pub(super) fn is_pending(&self) -> bool {
        self.pending.load(Ordering::Acquire)
    }

    pub(super) fn claim(&self) -> bool {
        self.pending.swap(false, Ordering::AcqRel)
    }

    #[tracing::instrument(name = "mcp.runtime.refresh_wait", skip_all)]
    pub(super) async fn acquire(&self) -> Result<SemaphorePermit<'_>, AcquireError> {
        self.gate.acquire().await
    }

    /// Recheck local Lico on demand and invalidate when its listener comes or goes.
    #[cfg(target_os = "trueos")]
    pub(super) async fn probe_lico(&self) -> bool {
        let mut state = self.lico.lock().await;
        if state.is_some_and(|(last_probe, _)| {
            last_probe.elapsed() < std::time::Duration::from_millis(250)
        }) {
            return false;
        }
        let listening = codex_mcp::trueos_lico::endpoint_is_listening().await;
        if listening != state.is_some_and(|(_, listening)| listening) {
            self.invalidate();
        }
        *state = Some((tokio::time::Instant::now(), listening));
        listening
    }

    pub(super) fn close(&self) {
        self.gate.close();
    }
}

/// Restores a claimed refresh when its task is cancelled before publication.
pub(super) struct McpRefreshInvalidationGuard<'a> {
    pub(super) refresh: &'a McpRefresh,
    pub(super) published: bool,
}

impl Drop for McpRefreshInvalidationGuard<'_> {
    fn drop(&mut self) {
        if !self.published {
            self.refresh.invalidate();
        }
    }
}

#[cfg(test)]
#[path = "mcp_refresh_tests.rs"]
mod tests;
