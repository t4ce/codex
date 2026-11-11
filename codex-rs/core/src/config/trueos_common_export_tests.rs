use super::*;
use crate::safety::PatchSandboxRoute;
use crate::safety::SafetyCheck;
use crate::safety::assess_write_safety;
use codex_protocol::permissions::FileSystemSandboxPolicyContext;
use pretty_assertions::assert_eq;

async fn load_trueos_defaults(
    sandbox_mode: Option<SandboxMode>,
    default_permissions: Option<&str>,
) -> std::io::Result<(Config, TempDir)> {
    load_trueos_defaults_with_overrides(ConfigOverrides {
        sandbox_mode,
        default_permissions: default_permissions.map(str::to_owned),
        ..Default::default()
    })
    .await
}

async fn load_trueos_defaults_with_overrides(
    mut overrides: ConfigOverrides,
) -> std::io::Result<(Config, TempDir)> {
    let directory = TempDir::new()?;
    let cwd = directory.path().join("workspace");
    std::fs::create_dir(&cwd)?;
    let defaults: ConfigToml = toml::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../config/defaults.trueos.toml"
    )))
    .expect("packaged TRUEOS defaults must be valid Codex config");
    overrides.cwd = Some(cwd);
    overrides.approval_policy = Some(AskForApproval::Never);
    let config = Config::load_from_base_config_with_overrides(
        defaults,
        overrides,
        directory.path().join("codex-home").abs(),
    )
    .await?;
    Ok((config, directory))
}

fn check_write(config: &Config, path: &str) -> std::io::Result<(bool, SafetyCheck)> {
    let cwd = PathUri::from_abs_path(&config.cwd);
    let roots = config.effective_workspace_roots();
    let context = FileSystemSandboxPolicyContext {
        cwd: &cwd,
        workspace_roots: &roots,
        user_home_dir: None,
        temporary_directories: None,
    };
    let policy = config.permissions.file_system_sandbox_policy();
    // TRUEOS uses native filesystem policy checks rather than a Unix helper.
    // Use the same matching route as write_file/copy_file authorization.
    let matching = PatchSandboxRoute::ExecutorManaged.prepare_matching(&policy, &context)?;
    let target = PathUri::from_host_native_path(path)?;
    let writable = matching.can_write_path(&target)?;
    let assessment = assess_write_safety(
        writable,
        config.permissions.approval_policy.value(),
        config.permissions.permission_profile(),
        &matching,
    )?;
    Ok((writable, assessment))
}

#[tokio::test]
async fn trueos_defaults_common_export_autoapproves_under_workspace_write() -> std::io::Result<()> {
    let (config, _directory) =
        load_trueos_defaults(Some(SandboxMode::WorkspaceWrite), None).await?;
    for path in ["/apps/common/house.png", "/apps/common/exports/weather.csv"] {
        assert_eq!(
            check_write(&config, path)?,
            (true, SafetyCheck::AutoApprove),
            "the trusted common export root must pass both policy and approval: {path}"
        );
    }
    for path in [
        "/apps/private/house.png",
        "/apps/common-other/house.png",
        "/apps/common/../private/house.png",
    ] {
        let (writable, assessment) = check_write(&config, path)?;
        assert!(
            !writable,
            "common export must not widen another root: {path}"
        );
        assert!(matches!(assessment, SafetyCheck::Reject { .. }));
    }
    Ok(())
}

#[tokio::test]
async fn trueos_defaults_common_export_respects_explicit_read_only() -> std::io::Result<()> {
    for (sandbox_mode, profile) in [
        (Some(SandboxMode::ReadOnly), None),
        (None, Some(BUILT_IN_PERMISSION_PROFILE_READ_ONLY)),
    ] {
        let (config, _directory) = load_trueos_defaults(sandbox_mode, profile).await?;
        let (writable, assessment) = check_write(&config, "/apps/common/house.png")?;
        assert!(
            !writable,
            "explicit read-only must keep FFA exports read-only"
        );
        assert!(matches!(assessment, SafetyCheck::Reject { .. }));
    }
    Ok(())
}

#[tokio::test]
async fn trueos_defaults_common_export_selected_or_saved_workspace_uses_platform_preset()
-> std::io::Result<()> {
    for overrides in [
        ConfigOverrides {
            default_permissions: Some(BUILT_IN_PERMISSION_PROFILE_WORKSPACE.to_owned()),
            ..Default::default()
        },
        ConfigOverrides {
            persisted_permission_profile_id: Some(BUILT_IN_PERMISSION_PROFILE_WORKSPACE.to_owned()),
            ..Default::default()
        },
    ] {
        let (config, _directory) = load_trueos_defaults_with_overrides(overrides).await?;
        let (writable, assessment) = check_write(&config, "/apps/common/house.png")?;
        assert_eq!(writable, cfg!(target_os = "trueos"));
        if cfg!(target_os = "trueos") {
            assert_eq!(assessment, SafetyCheck::AutoApprove);
        } else {
            assert!(matches!(assessment, SafetyCheck::Reject { .. }));
        }
    }
    Ok(())
}

#[test]
fn trueos_defaults_common_export_native_workspace_preset_is_narrow_and_shared_by_approval()
-> std::io::Result<()> {
    let cwd = PathUri::parse("file:///apps/codex/workspace").expect("valid workspace URI");
    let roots = vec![cwd.clone()];
    let context = FileSystemSandboxPolicyContext {
        cwd: &cwd,
        workspace_roots: &roots,
        user_home_dir: None,
        temporary_directories: None,
    };
    for native_trueos in [false, true] {
        let profile = super::super::permissions::builtin_permission_profile_for_platform(
            BUILT_IN_PERMISSION_PROFILE_WORKSPACE,
            None,
            native_trueos,
        )
        .expect("workspace is a built-in profile");
        let policy = profile.file_system_sandbox_policy();
        let matching = PatchSandboxRoute::ExecutorManaged.prepare_matching(&policy, &context)?;
        for path in ["/apps/common/house.png", "/apps/common/exports/weather.csv"] {
            let target = PathUri::from_host_native_path(path)?;
            let writable = matching.can_write_path(&target)?;
            assert_eq!(writable, native_trueos);
            let assessment =
                assess_write_safety(writable, AskForApproval::Never, &profile, &matching)?;
            if native_trueos {
                assert_eq!(assessment, SafetyCheck::AutoApprove);
            } else {
                assert!(matches!(assessment, SafetyCheck::Reject { .. }));
            }
        }
        for path in ["/apps/private/house.png", "/apps/common-other/house.png"] {
            assert!(!matching.can_write_path(&PathUri::from_host_native_path(path)?)?);
        }
    }
    let read_only = super::super::permissions::builtin_permission_profile_for_platform(
        BUILT_IN_PERMISSION_PROFILE_READ_ONLY,
        None,
        true,
    )
    .expect("read-only is a built-in profile");
    assert!(!read_only.file_system_sandbox_policy().can_write_path(
        &PathUri::parse("file:///apps/common/house.png").expect("valid common export URI"),
        &context,
    ));
    assert!(
        super::super::permissions::builtin_permission_profile_for_platform(
            "custom-restricted",
            None,
            true,
        )
        .is_none()
    );
    Ok(())
}

#[tokio::test]
async fn trueos_defaults_common_export_survives_saved_unnamed_workspace_profile()
-> std::io::Result<()> {
    let (config, _directory) = load_trueos_defaults_with_overrides(ConfigOverrides {
        persisted_permission_profile: Some(PermissionProfile::workspace_write()),
        ..Default::default()
    })
    .await?;
    assert_eq!(
        check_write(&config, "/apps/common/house.png")?,
        (true, SafetyCheck::AutoApprove),
        "saved symbolic workspace permissions must include configured runtime roots"
    );
    Ok(())
}
