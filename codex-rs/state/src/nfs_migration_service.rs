//! TRUEOS SQLite images and journal transitions belong to the resident service.
//! Client paths are logical database names, not local files or raw SQLite handles.

use std::path::Path;
use std::path::PathBuf;

#[derive(Debug)]
pub struct NfsDbMigration;

pub enum NfsDbMigrationOutcome {
    Missing,
    AlreadyDelete,
    Migrated { backup_directory: PathBuf },
}

fn unsupported() -> anyhow::Error {
    std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "TRUEOS SQLite databases are service-owned; client-side NFS inspection and migration are unavailable",
    ).into()
}

impl NfsDbMigration {
    pub async fn needs_migration(_source: &Path) -> anyhow::Result<bool> {
        Err(unsupported())
    }

    pub fn lock(_sqlite_home: &Path) -> anyhow::Result<Self> {
        Err(unsupported())
    }

    pub fn needs_permission_confirmation(_source: &Path) -> anyhow::Result<bool> {
        Err(unsupported())
    }

    pub async fn migrate(
        &mut self,
        _source: &Path,
        _staging_dir: &Path,
    ) -> anyhow::Result<NfsDbMigrationOutcome> {
        Err(unsupported())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn native_migration_rejects_client_file_operations_without_mutating_paths() {
        let home = tempfile::tempdir().unwrap();
        let source = home.path().join("state.sqlite");
        std::fs::write(&source, b"service-owned logical database").unwrap();
        let assert_unsupported = |error: anyhow::Error| {
            assert_eq!(
                error.downcast_ref::<std::io::Error>().unwrap().kind(),
                std::io::ErrorKind::Unsupported
            );
            assert!(error.to_string().contains("service-owned"));
        };
        assert_unsupported(NfsDbMigration::needs_migration(&source).await.unwrap_err());
        assert_unsupported(NfsDbMigration::needs_permission_confirmation(&source).unwrap_err());
        assert_unsupported(NfsDbMigration::lock(home.path()).unwrap_err());
        let result = NfsDbMigration.migrate(&source, home.path()).await;
        assert_unsupported(
            result
                .err()
                .expect("native migration cannot rewrite an image"),
        );
        assert_eq!(
            std::fs::read(&source).unwrap(),
            b"service-owned logical database"
        );
        assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 1);
    }
}
