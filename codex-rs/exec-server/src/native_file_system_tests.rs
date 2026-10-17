use super::*;
use crate::protocol::{FsCreateDirectoryParams, FsReadFileParams, FsWriteFileParams};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use codex_protocol::models::PermissionProfile;
use codex_protocol::permissions::{
    FileSystemAccessMode, FileSystemSandboxEntry, FileSystemSandboxPolicy, NetworkSandboxPolicy,
};

fn uri(path: &std::path::Path) -> PathUri {
    PathUri::from_host_native_path(path).expect("native path")
}

fn context(root: &std::path::Path) -> FileSystemSandboxContext {
    let policy = FileSystemSandboxPolicy::restricted(vec![FileSystemSandboxEntry {
        path: uri(root).into(),
        access: FileSystemAccessMode::Write,
        missing_path_behavior: None,
    }]);
    FileSystemSandboxContext::from_permission_profile(
        PermissionProfile::from_runtime_permissions(&policy, NetworkSandboxPolicy::Restricted),
        uri(root),
    )
}

fn write(path: PathUri, data: &str) -> FsHelperRequest {
    FsHelperRequest::WriteFile(FsWriteFileParams {
        create_new: None,
        expected_sha256: None,
        path,
        data_base64: STANDARD.encode(data),
        follow_symlinks: Some(false),
        sandbox: None,
    })
}

#[tokio::test]
async fn private_workspace_file_can_be_created_updated_and_read_back() {
    let temp = tempfile::tempdir().expect("workspace");
    let sandbox = context(temp.path());
    let path = uri(&temp.path().join("hello.txt"));
    run(&sandbox, write(path.clone(), "hello codex"))
        .await
        .expect("write file");
    assert_eq!(
        std::fs::read_to_string(temp.path().join("hello.txt")).expect("bytes"),
        "hello codex"
    );
    let read = run(
        &sandbox,
        FsHelperRequest::ReadFile(FsReadFileParams {
            path: path.clone(),
            follow_symlinks: Some(false),
            sandbox: None,
        }),
    )
    .await
    .expect("read file")
    .expect_read_file()
    .expect("read response");
    assert_eq!(
        STANDARD.decode(read.data_base64).expect("decode"),
        b"hello codex"
    );
    run(&sandbox, write(path, "changed"))
        .await
        .expect("update file");
    assert_eq!(
        std::fs::read_to_string(temp.path().join("hello.txt")).expect("bytes"),
        "changed"
    );
}

#[tokio::test]
async fn outside_workspace_and_protected_metadata_writes_remain_denied() {
    let temp = tempfile::tempdir().expect("base");
    let root = temp.path().join("workspace");
    std::fs::create_dir(&root).expect("root");
    let sandbox = context(&root);
    for path in [
        temp.path().join("outside.txt"),
        root.join(".codex/config.toml"),
        root.join(".git/config"),
    ] {
        assert!(run(&sandbox, write(uri(&path), "denied")).await.is_err());
        assert!(!path.exists());
    }
}

#[tokio::test]
async fn read_only_policy_is_enforced_without_a_platform_helper() {
    let temp = tempfile::tempdir().expect("workspace");
    let sandbox = FileSystemSandboxContext::from_permission_profile(
        PermissionProfile::from_runtime_permissions(
            &FileSystemSandboxPolicy::read_only(),
            NetworkSandboxPolicy::Restricted,
        ),
        uri(temp.path()),
    );
    assert!(
        run(
            &sandbox,
            write(uri(&temp.path().join("hello.txt")), "denied")
        )
        .await
        .is_err()
    );
    assert!(!temp.path().join("hello.txt").exists());
}

#[tokio::test]
async fn recursive_directory_creation_checks_missing_ancestors() {
    let temp = tempfile::tempdir().expect("base");
    let root = temp.path().join("workspace");
    std::fs::create_dir(&root).expect("root");
    let sandbox = context(&root);
    let target = root.join("new/deep");
    run(
        &sandbox,
        FsHelperRequest::CreateDirectory(FsCreateDirectoryParams {
            path: uri(&target),
            recursive: Some(true),
            follow_symlinks: Some(false),
            sandbox: None,
        }),
    )
    .await
    .expect("create private directories");
    assert!(target.is_dir());
    let missing_root = temp.path().join("outside/only-allowed-child");
    let sandbox = context(&missing_root);
    assert!(
        run(
            &sandbox,
            FsHelperRequest::CreateDirectory(FsCreateDirectoryParams {
                path: uri(&missing_root),
                recursive: Some(true),
                follow_symlinks: Some(false),
                sandbox: None,
            })
        )
        .await
        .is_err()
    );
    assert!(!temp.path().join("outside").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn symbolic_and_hard_link_aliases_cannot_modify_an_outside_file() {
    let temp = tempfile::tempdir().expect("base");
    let root = temp.path().join("workspace");
    std::fs::create_dir(&root).expect("root");
    let outside = temp.path().join("secret");
    std::fs::write(&outside, "original").expect("outside");
    let sandbox = context(&root);
    for (name, symbolic) in [("symlink", true), ("hardlink", false)] {
        let alias = root.join(name);
        if symbolic {
            std::os::unix::fs::symlink(&outside, &alias).expect("symlink");
        } else {
            std::fs::hard_link(&outside, &alias).expect("hard link");
        }
        assert!(run(&sandbox, write(uri(&alias), "changed")).await.is_err());
        assert_eq!(
            std::fs::read_to_string(&outside).expect("outside bytes"),
            "original"
        );
    }
}

#[test]
fn native_adapter_rejects_foreign_paths_and_unchecked_recursive_mutation() {
    let temp = tempfile::tempdir().expect("workspace");
    let sandbox = context(temp.path());
    #[cfg(unix)]
    assert!(
        check_path(
            &sandbox,
            &PathUri::parse("file://server/share/file").expect("foreign URI"),
            true
        )
        .is_err()
    );
    assert!(require_recursive_access(&sandbox, true).is_err());
}

#[tokio::test]
async fn create_only_guard_survives_permission_adapter_and_helper_protocol() {
    let temp = tempfile::tempdir().expect("workspace");
    let sandbox = context(temp.path());
    let path = uri(&temp.path().join("draft.rs"));
    let create = |content: &str| {
        let mut request = write(path.clone(), content);
        if let FsHelperRequest::WriteFile(params) = &mut request {
            params.create_new = Some(true);
        }
        request
    };
    run(&sandbox, create("fn unfinished("))
        .await
        .expect("first create-only write");
    assert!(run(&sandbox, create("replacement")).await.is_err());
    assert_eq!(
        std::fs::read(temp.path().join("draft.rs")).expect("bytes"),
        b"fn unfinished("
    );
}
