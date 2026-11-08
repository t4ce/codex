use super::read_sensitive_file_to_string;
use tempfile::TempDir;

#[test]
fn missing_config_probe_returns_not_found() {
    let directory = TempDir::new().expect("temporary directory");
    let path = directory.path().join("absent-config.toml");
    let error = super::open_sync(&path, crate::protocol::FsOpenMode::Read)
        .expect_err("missing config file");
    assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    assert!(!path.exists());
}

#[tokio::test]
async fn missing_sensitive_config_probe_returns_not_found() {
    let directory = TempDir::new().expect("temporary directory");
    let path = directory.path().join("absent-role.toml");
    let error = read_sensitive_file_to_string(&path)
        .await
        .expect_err("missing sensitive config file");
    assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    assert!(!path.exists());
}

#[test]
fn regular_open_rejects_directory_without_altering_it() {
    let directory = TempDir::new().expect("temporary directory");
    let error = super::open_sync(directory.path(), crate::protocol::FsOpenMode::Read)
        .expect_err("directory is not a regular file");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    assert!(directory.path().is_dir());
}

#[tokio::test]
async fn read_sensitive_file_reads_regular_file() {
    let directory = TempDir::new().expect("temporary directory");
    let path = directory.path().join("role.toml");
    tokio::fs::write(&path, "developer_instructions = 'stay focused'")
        .await
        .expect("write regular file");

    assert_eq!(
        read_sensitive_file_to_string(&path)
            .await
            .expect("read regular file"),
        "developer_instructions = 'stay focused'",
    );
}

#[tokio::test]
async fn read_sensitive_file_rejects_directory() {
    let directory = TempDir::new().expect("temporary directory");

    assert!(
        read_sensitive_file_to_string(directory.path())
            .await
            .is_err()
    );
}

#[cfg(all(any(unix, windows), not(target_os = "trueos")))]
#[tokio::test]
async fn read_sensitive_file_rejects_symlink() {
    let directory = TempDir::new().expect("temporary directory");
    let target = directory.path().join("target.toml");
    let link = directory.path().join("role.toml");
    tokio::fs::write(&target, "model_provider = 'attacker'")
        .await
        .expect("write symlink target");

    #[cfg(unix)]
    std::os::unix::fs::symlink(&target, &link).expect("create symlink");
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(&target, &link).expect("create symlink");

    assert!(read_sensitive_file_to_string(&link).await.is_err());
}
