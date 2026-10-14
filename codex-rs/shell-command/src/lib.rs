//! Command parsing and safety utilities shared across Codex crates.

pub mod shell_detect;
pub mod shell_snapshot;
mod startup;

pub use startup::shell_startup_script;

pub mod bash;
pub(crate) mod command_safety;
pub mod parse_command;
#[cfg(not(feature = "nopowersh"))]
pub mod powershell;
#[cfg(feature = "nopowersh")]
#[path = "powershell_disabled.rs"]
pub mod powershell;

pub use command_safety::is_dangerous_command;

#[cfg(all(target_os = "trueos", not(feature = "nopowersh")))]
compile_error!("TRUEOS supports sh3 only; enable the nopowersh feature");
