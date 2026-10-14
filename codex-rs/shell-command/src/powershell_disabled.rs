//! Compatibility API without PowerShell: scripts stay opaque and discovery never spawns processes.
use codex_utils_paths::absolute_path::AbsolutePathBuf;
pub const UTF8_OUTPUT_PREFIX: &str = "";
pub fn prefix_powershell_script_with_utf8(command: &[String]) -> Vec<String> {
    command.to_vec()
}
pub fn extract_powershell_command(_command: &[String]) -> Option<(&str, &str)> {
    None
}
pub fn parse_powershell_command_into_plain_commands(
    _command: &[String],
) -> Option<Vec<Vec<String>>> {
    None
}
pub fn parse_powershell_script_into_plain_commands(_script: &str) -> Option<Vec<Vec<String>>> {
    None
}
pub fn try_find_powershell_executable_blocking() -> Option<AbsolutePathBuf> {
    None
}
pub fn try_find_pwsh_executable_blocking() -> Option<AbsolutePathBuf> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn powershell_stays_opaque_and_is_never_discovered() {
        let command = ["pwsh", "-Command", "Get-Content file; Remove-Item file"].map(str::to_owned);
        assert_eq!(extract_powershell_command(&command), None);
        assert_eq!(
            crate::parse_command::parse_command_impl(&command),
            vec![codex_protocol::parse_command::ParsedCommand::Unknown {
                cmd: crate::parse_command::shlex_join(&command),
            }],
        );
        assert_eq!(parse_powershell_command_into_plain_commands(&command), None);
        assert_eq!(
            parse_powershell_script_into_plain_commands(&command[2]),
            None
        );
        assert_eq!(prefix_powershell_script_with_utf8(&command), command);
        assert_eq!(try_find_pwsh_executable_blocking(), None);
        assert_eq!(try_find_powershell_executable_blocking(), None);
    }
}
