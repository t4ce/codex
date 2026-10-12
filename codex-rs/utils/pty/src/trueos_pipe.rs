//! TRUEOS command sessions use native launch scripts rather than Unix pipes.
use crate::SpawnedProcess;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::Path;

pub async fn spawn_process(
    _program: impl AsRef<OsStr>,
    _args: &[String],
    _cwd: &Path,
    _env: &HashMap<String, String>,
    _arg0: &Option<String>,
    _inherited_fds: &[i32],
) -> anyhow::Result<SpawnedProcess> {
    anyhow::bail!(
        "TRUEOS executes Shell3 launch scripts; use spawn_shell3_script instead of Unix program/argv spawning"
    )
}

pub async fn spawn_process_no_stdin(
    program: impl AsRef<OsStr>,
    args: &[String],
    cwd: &Path,
    env: &HashMap<String, String>,
    arg0: &Option<String>,
    inherited_fds: &[i32],
) -> anyhow::Result<SpawnedProcess> {
    spawn_process(program, args, cwd, env, arg0, inherited_fds).await
}
