//! Native object operations in TRUEOSFS's link-free namespace. The kernel
//! resolves and authorizes complete paths while acquiring each file capability;
//! there is no Unix directory descriptor or host ownership interpretation.
use crate::FileMetadata;
use std::fs::{File, Metadata, OpenOptions};
use std::io;
use std::path::{Component, Path, PathBuf};

fn normalized(path: &Path) -> io::Result<()> {
    if !path.is_absolute()
        || path.components().any(|part| matches!(part, Component::ParentDir | Component::CurDir | Component::Prefix(_)))
    {
        return Err(super::invalid_output("native filesystem operations require a normalized absolute path"));
    }
    let text = path.to_str().ok_or_else(|| super::invalid_output("native filesystem paths require UTF-8"))?;
    if text.as_bytes().iter().any(|byte| *byte == 0 || *byte == b'\\' || *byte < 32) {
        return Err(super::invalid_output("invalid native filesystem path"));
    }
    Ok(())
}

fn identity(metadata: &Metadata) -> io::Result<(u32, u64)> {
    Ok((metadata.root_id(), metadata.file_id().ok_or_else(|| io::Error::new(io::ErrorKind::Unsupported, "native object identity is unavailable"))?))
}

pub(super) fn output_path(path: &Path) -> io::Result<PathBuf> {
    let text = path.as_os_str().as_encoded_bytes();
    if matches!(text, b"/dev/stdout" | b"/dev/stderr")
        || text.starts_with(b"/dev/fd/") || text.starts_with(b"/proc/self/fd/")
    {
        return Err(io::Error::new(io::ErrorKind::Unsupported, "native console streams cannot be opened as Unix file descriptors"));
    }
    let path = std::path::absolute(path)?;
    normalized(&path)?;
    Ok(path)
}

pub(super) fn verify_output_path(file: &File, path: &Path) -> io::Result<()> {
    normalized(path)?;
    if identity(&file.metadata()?)? != identity(&std::fs::symlink_metadata(path)?)? {
        return Err(super::invalid_output("output path was replaced"));
    }
    Ok(())
}

pub(super) fn reject_multiple_links(file: &File) -> io::Result<()> {
    // Native creation and rename cannot create hard links. Require an actual
    // identifiable regular object rather than inventing an nlink count.
    let metadata = file.metadata()?;
    identity(&metadata)?;
    if !metadata.is_file() {
        return Err(super::invalid_output("path is not a regular native file"));
    }
    Ok(())
}

pub(super) fn open_output(path: &Path) -> io::Result<File> {
    normalized(path)?;
    let file = OpenOptions::new().write(true).create(true).open(path)?;
    reject_multiple_links(&file)?;
    Ok(file)
}

pub(super) fn open_file_sync(path: &Path) -> io::Result<File> {
    normalized(path)?;
    let file = File::open(path)?;
    reject_multiple_links(&file)?;
    Ok(file)
}

pub(super) async fn write_file(path: PathBuf, contents: Vec<u8>, create_new: bool) -> io::Result<()> {
    normalized(&path)?;
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true);
    if create_new { options.create_new(true); } else { options.create(true); }
    let mut file = options.open(&path).await?;
    if !file.metadata().await?.is_file() {
        return Err(super::invalid_output("path is not a regular native file"));
    }
    file.set_len(0).await?;
    tokio::io::AsyncWriteExt::write_all(&mut file, &contents).await
}

pub(super) async fn metadata(path: PathBuf) -> io::Result<FileMetadata> {
    normalized(&path)?;
    let metadata = tokio::fs::symlink_metadata(path).await?;
    Ok(FileMetadata {
        content_type: None,
        is_directory: metadata.is_dir(),
        is_file: metadata.is_file(),
        is_symlink: false,
        size: metadata.len(),
        // The protocol already uses zero for unavailable wall-clock times.
        created_at_ms: 0,
        modified_at_ms: 0,
    })
}

pub(super) async fn create_directory(path: PathBuf, recursive: bool) -> io::Result<()> {
    normalized(&path)?;
    if recursive { tokio::fs::create_dir_all(path).await } else { tokio::fs::create_dir(path).await }
}

pub(super) async fn remove(path: PathBuf, recursive: bool, force: bool) -> io::Result<()> {
    normalized(&path)?;
    if recursive {
        return Err(io::Error::new(io::ErrorKind::Unsupported, "recursive no-follow removal is unsupported"));
    }
    let metadata = match tokio::fs::symlink_metadata(&path).await {
        Ok(metadata) => metadata,
        Err(error) if force && error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let result = if metadata.is_dir() { tokio::fs::remove_dir(path).await } else { tokio::fs::remove_file(path).await };
    match result {
        Err(error) if force && error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}
