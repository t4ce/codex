//! Install the optional native XERJ MCP executable only when its local launch is requested.

use std::ffi::OsStr;
use std::io;

fn uses_packaged_xerj(program: &OsStr) -> bool {
    // Explicit paths and replacement commands belong to the user's configuration.
    program == OsStr::new("xerj") || program == OsStr::new("xerj.bp")
}

fn xerj_installed(bytes: &[u8]) -> io::Result<bool> {
    let catalog: serde_json::Value = serde_json::from_slice(bytes).map_err(io::Error::other)?;
    let apps = catalog["apps"]
        .as_array()
        .ok_or_else(|| io::Error::other("invalid AppDB catalog"))?;
    Ok(apps.iter().any(|app| app["archive"] == "xerj.bp"))
}

#[cfg(target_os = "trueos")]
pub(crate) async fn ensure_installed(program: &OsStr) -> io::Result<()> {
    use std::time::Duration;

    if !uses_packaged_xerj(program) {
        return Ok(());
    }
    static INSTALL: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);
    let _permit = INSTALL.acquire().await.map_err(io::Error::other)?;
    let installed = async || {
        let bytes = trueos::async_fs::read_file(b"vFile:appdb")
            .await
            .map_err(|code| io::Error::other(format!("read AppDB for XERJ ({code})")))?;
        xerj_installed(&bytes)
    };
    if installed().await? {
        return Ok(());
    }
    // Install imports xerj.bp without starting a detached engine or MCP process.
    // The normal child transport remains the sole owner of the MCP lifetime.
    trueos::vshell::launch_with_destination(
        "apps",
        "install xerj-service\n",
        trueos::vshell::LaunchDestination::Headless,
    )
    .map_err(|code| io::Error::other(format!("request XERJ Blueprint install ({code})")))?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(110);
    loop {
        if installed().await? {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "XERJ Blueprint installation timed out; disable with -c mcp_servers.xerj.enabled=false",
            ));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installation_applies_only_to_the_native_packaged_command() {
        for program in ["xerj", "xerj.bp"] {
            assert!(uses_packaged_xerj(OsStr::new(program)));
        }
        for program in ["custom-xerj", "/apps/xerj", "./xerj.bp", "robomcp"] {
            assert!(!uses_packaged_xerj(OsStr::new(program)));
        }
    }

    #[test]
    fn installation_readiness_requires_the_imported_executable_archive() {
        assert!(xerj_installed(br#"{"apps":[{"archive":"xerj.bp"}]}"#).unwrap());
        assert!(!xerj_installed(br#"{"apps":[{"archive":"xerj-service.bp"}]}"#).unwrap());
        assert!(!xerj_installed(br#"{"apps":[]}"#).unwrap());
        assert!(xerj_installed(b"{}").is_err());
    }
}
