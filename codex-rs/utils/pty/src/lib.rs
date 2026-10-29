#[cfg(all(target_os = "trueos", not(feature = "sh3")))]
compile_error!(
    "TRUEOS command sessions require the `sh3` feature; enable `trueos-blueprint` on the CLI for Blueprint builds"
);

mod child;
pub use child::Child;
mod child_command;
pub use child_command::ChildStdin;
pub use child_command::Command;
pub use child_command::DescriptorPolicy;
pub use child_command::ProcessMode;
pub use child_command::SpawnFallback;
mod child_fds;
#[cfg(target_os = "linux")]
mod linux_fds;
#[cfg_attr(all(target_os = "trueos", feature = "sh3"), path = "trueos_pipe.rs")]
pub mod pipe;
mod process;
pub mod process_group;
#[cfg_attr(all(target_os = "trueos", feature = "sh3"), path = "trueos.rs")]
pub mod pty;
#[cfg(all(test, not(target_os = "trueos")))]
#[path = "trueos.rs"]
mod trueos_backend_tests;
pub use pty::ChildFds;
#[cfg(all(target_os = "trueos", feature = "sh3"))]
pub use pty::install_shell3_backend;
#[cfg(all(target_os = "trueos", feature = "sh3"))]
pub use pty::script_stdin_open as shell3_stdin_open;
#[cfg(all(target_os = "trueos", feature = "sh3"))]
pub use pty::spawn_script as spawn_shell3_script;
#[cfg(test)]
mod tests;
#[cfg(all(unix, not(target_os = "trueos")))]
mod unix_io;
#[cfg(windows)]
mod win;
#[cfg(windows)]
mod windows_input;

pub const DEFAULT_OUTPUT_BYTES_CAP: usize = 1024 * 1024;

/// Spawn a non-interactive process using regular pipes for stdin/stdout/stderr.
pub use pipe::spawn_process as spawn_pipe_process;
/// Spawn a non-interactive process using regular pipes, but close stdin immediately.
pub use pipe::spawn_process_no_stdin as spawn_pipe_process_no_stdin;
/// Driver-backed process adapter used by integrations with their own process transport.
pub use process::ProcessDriver;
/// Handle for interacting with a spawned process (PTY or pipe).
pub use process::ProcessHandle;
/// Process signal supported by spawned-process handles.
pub use process::ProcessSignal;
/// Bundle of process handles plus split output and exit receivers returned by spawn helpers.
pub use process::SpawnedProcess;
/// Terminal size in character cells used for PTY spawn and resize operations.
pub use process::TerminalSize;
/// Combine stdout/stderr receivers into a single broadcast receiver.
pub use process::combine_output_receivers;
/// Adapt an externally-driven process into the standard spawned-process handle.
pub use process::spawn_from_driver;
/// Backwards-compatible alias for ProcessHandle.
pub type ExecCommandSession = ProcessHandle;
/// Backwards-compatible alias for SpawnedProcess.
pub type SpawnedPty = SpawnedProcess;
/// Report whether ConPTY is available on this platform (Windows only).
pub use pty::conpty_supported;
/// Spawn a process attached to a PTY for interactive use.
pub use pty::spawn_process as spawn_pty_process;
#[cfg(windows)]
pub use win::JobObject;
#[cfg(windows)]
pub use win::PsuedoCon;
#[cfg(windows)]
pub use win::conpty::RawConPty;
#[cfg(windows)]
pub use windows_input::WindowsTtyInputNormalizer;

#[cfg(target_os = "linux")]
mod spawn_helper;
#[cfg(target_os = "linux")]
mod spawn_helper_main;
#[cfg(target_os = "linux")]
pub use spawn_helper::init_spawn_helper;
#[cfg(all(test, target_os = "linux"))]
#[path = "spawn_helper_tests.rs"]
mod spawn_helper_tests;
