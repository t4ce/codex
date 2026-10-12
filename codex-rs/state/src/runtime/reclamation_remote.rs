//! The SQLite Blueprint owns serialization and page reclamation on TRUEOS.
//! No client file lock or dedicated SQLite connection is needed.
use crate::SqliteConfig;
use std::sync::Arc;
pub(crate) struct SqliteReclamationWorker;
impl SqliteReclamationWorker {
    pub(crate) fn spawn(_sqlite: SqliteConfig) -> Arc<Self> {
        Arc::new(Self)
    }
    pub(crate) async fn close(&self) {}
}
