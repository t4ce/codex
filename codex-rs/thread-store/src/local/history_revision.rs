//! Validates snapshots of append-only local rollouts before reusing them for resume.
//!
//! Capture before and after reading; any change or unavailable metadata disables reuse. The
//! selected path prevents reuse for a different rollout. Validate under writer ownership,
//! before opening the recorder can materialize a compressed file.

use std::path::Path;

pub(super) async fn read(path: &Path) -> Option<String> {
    let path = codex_rollout::existing_rollout_path(path).await?;
    let path = tokio::fs::canonicalize(path).await.ok()?;
    let metadata = tokio::fs::metadata(&path).await.ok()?;
    #[cfg(target_os = "trueos")]
    {
        // TRUEOSFS has a committed record revision rather than wall-clock times.
        // A rewrite changes it even when the file ID and byte length stay equal.
        return serde_json::to_string(&(
            "trueos-local-v1",
            path,
            metadata.root_id(),
            metadata.file_id()?,
            metadata.len(),
            metadata.revision()?,
        ))
        .ok();
    }
    #[cfg(not(target_os = "trueos"))]
    {
        let modified = metadata.modified().ok()?;
        let created = metadata.created().ok();
        #[cfg(unix)]
        let identity = {
            use std::os::unix::fs::MetadataExt;
            Some((
                metadata.dev(),
                metadata.ino(),
                metadata.ctime(),
                metadata.ctime_nsec(),
            ))
        };
        #[cfg(not(unix))]
        let identity: Option<(u64, u64, i64, i64)> = None;
        serde_json::to_string(&("local", path, metadata.len(), modified, created, identity)).ok()
    }
}
