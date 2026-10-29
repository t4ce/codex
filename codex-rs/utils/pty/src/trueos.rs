//! Shell3 launch-script transport. No Unix PTY or process model is used here.
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::Result;
use anyhow::anyhow;
use anyhow::bail;
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use trueos::command_session::Backend;
use trueos::command_session::Geometry;
use trueos::command_session::Launch;
use trueos::command_session::Session;
use trueos::command_session::Signal;
use trueos::command_session::State;
use trueos::command_session::Stream;

pub use crate::child_fds::ChildFds;
use crate::process::ProcessDriver;
use crate::process::ProcessSignal;
use crate::process::SpawnedProcess;
use crate::process::TerminalSize;
use crate::process::spawn_from_driver_with_signaler;

static BACKEND: OnceLock<Arc<dyn Backend>> = OnceLock::new();

/// Install the platform's Shell3 transport during application initialization.
/// A backend must own dedicated sessions and their exact lifetimes. Replacing
/// a backend while sessions are live is deliberately unsupported.
pub fn install_shell3_backend(backend: Arc<dyn Backend>) -> Result<()> {
    BACKEND
        .set(backend)
        .map_err(|_| anyhow!("Shell3 backend already installed"))
}

pub fn conpty_supported() -> bool {
    false
}

/// Nonempty captured command batches finish at EOF. Empty scripts deliberately
/// open reusable plain sessions; explicit terminals retain keyboard input.
pub fn script_stdin_open(script: &str, tty: bool) -> bool {
    tty || script.is_empty()
}

/// Legacy Unix launches are rejected; argv must not be silently reinterpreted
/// as typed Shell3 input, and Unix cwd/env/FD settings must not be discarded.
pub async fn spawn_process(
    _program: &str,
    _args: &[String],
    _cwd: &Path,
    _env: &HashMap<String, String>,
    _arg0: &Option<String>,
    _size: TerminalSize,
    _inherited_fds: ChildFds<'_>,
) -> Result<SpawnedProcess> {
    bail!(
        "TRUEOS executes Shell3 launch scripts; use spawn_shell3_script instead of Unix program/argv spawning"
    )
}

/// Submit native script text unchanged. An explicit terminal retains Tab mode
/// selection; None requests native command lines and captured text output.
pub async fn spawn_script(
    script: &str,
    size: Option<TerminalSize>,
    stdin_open: bool,
) -> Result<SpawnedProcess> {
    #[cfg(target_os = "trueos")]
    let backend = BACKEND.get_or_init(|| Arc::new(trueos::command_session::KernelBackend));
    #[cfg(not(target_os = "trueos"))]
    let backend = BACKEND
        .get()
        .ok_or_else(|| anyhow!("Shell3 command-session backend is not installed"))?;
    spawn_with_backend(&**backend, script, size, stdin_open).await
}

async fn spawn_with_backend(
    backend: &dyn Backend,
    script: &str,
    size: Option<TerminalSize>,
    stdin_open: bool,
) -> Result<SpawnedProcess> {
    let session = Arc::new(Mutex::new(Session::open(
        backend,
        Launch {
            script: script.to_owned(),
            terminal: size.map(|size| Geometry {
                rows: size.rows,
                cols: size.cols,
            }),
            stdin_open,
        },
    )?));
    if !stdin_open {
        lock(&session)?.transport().close_stdin()?;
    }
    let (stdin_tx, mut stdin_rx) = mpsc::channel::<Vec<u8>>(64);
    let (stdout_tx, stdout_rx) = broadcast::channel(256);
    let (stderr_tx, stderr_rx) = broadcast::channel(256);
    let (exit_tx, exit_rx) = oneshot::channel();
    let write_session = Arc::clone(&session);
    let writer_handle = tokio::spawn(async move {
        if stdin_open {
            while let Some(bytes) = stdin_rx.recv().await {
                // Captured input is command text, so Ctrl-C must use the
                // transport control channel. Explicit terminals consume keys.
                if size.is_none() && bytes == b"\x03" {
                    let result = lock(&write_session).and_then(|mut session| {
                        session
                            .transport()
                            .signal(Signal::Interrupt)
                            .map_err(Into::into)
                    });
                    if result.is_err() {
                        if let Ok(mut session) = lock(&write_session) {
                            let _ = session.transport().signal(Signal::Terminate);
                        }
                        return;
                    }
                    continue;
                }
                let mut sent = 0;
                while sent < bytes.len() {
                    let written = lock(&write_session).and_then(|mut session| {
                        session
                            .transport()
                            .write(&bytes[sent..])
                            .map_err(Into::into)
                    });
                    match written {
                        Ok(count) if count <= bytes.len() - sent => sent += count,
                        _ => {
                            if let Ok(mut session) = lock(&write_session) {
                                let _ = session.transport().signal(Signal::Terminate);
                            }
                            return;
                        }
                    }
                    if sent < bytes.len() {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                }
            }
            if let Ok(mut session) = lock(&write_session) {
                let _ = session.transport().close_stdin();
            }
        }
    });
    let read_session = Arc::clone(&session);
    tokio::spawn(async move {
        let mut buffer = vec![0; 8192];
        let code = loop {
            let read = lock(&read_session)
                .and_then(|mut session| session.transport().read(&mut buffer).map_err(Into::into));
            let read = match read {
                Ok(read) if read.len <= buffer.len() => read,
                _ => break -1,
            };
            if read.len != 0 {
                let output = buffer[..read.len].to_vec();
                let _ = match read.stream {
                    Stream::Stdout => stdout_tx.send(output),
                    Stream::Stderr => stderr_tx.send(output),
                };
            }
            if let State::Exited(code) = read.state {
                break code;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        // Driver readers drain until sender closure, including output on the exit read.
        drop(stdout_tx);
        drop(stderr_tx);
        let _ = exit_tx.send(code);
    });
    let signal_session = Arc::clone(&session);
    let resize_session = Arc::clone(&session);
    Ok(spawn_from_driver_with_signaler(
        ProcessDriver {
            writer_tx: stdin_tx,
            stdout_rx,
            stderr_rx: Some(stderr_rx),
            exit_rx,
            terminator: Some(Box::new(move || {
                if let Ok(mut session) = lock(&session) {
                    let _ = session.transport().signal(Signal::Terminate);
                }
            })),
            writer_handle: Some(writer_handle),
            resizer: size.map(|_| {
                Box::new(move |size: TerminalSize| {
                    lock(&resize_session)?.transport().resize(Geometry {
                        rows: size.rows,
                        cols: size.cols,
                    })?;
                    Ok(())
                }) as Box<dyn FnMut(TerminalSize) -> Result<()> + Send>
            }),
            #[cfg(windows)]
            tty: size.is_some(),
        },
        Some(Box::new(move |signal| {
            let signal = match signal {
                ProcessSignal::Interrupt => Signal::Interrupt,
            };
            lock(&signal_session)
                .and_then(|mut session| session.transport().signal(signal).map_err(Into::into))
                .map_err(std::io::Error::other)
        })),
    ))
}

fn lock(session: &Mutex<Session>) -> Result<std::sync::MutexGuard<'_, Session>> {
    session
        .lock()
        .map_err(|_| anyhow!("Shell3 session lock poisoned"))
}

#[cfg(test)]
#[path = "trueos_tests.rs"]
mod tests;
