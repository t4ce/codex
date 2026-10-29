//! Local TRUEOS help backed by the command catalog embedded at build time.
//! This provider never starts a shell session or invokes a process.

use std::collections::BTreeMap;

use codex_tools::JsonSchema;
use codex_tools::JsonToolOutput;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolName;
use codex_tools::ToolSpec;
use serde::Deserialize;
use serde_json::json;

use crate::function_tool::FunctionCallError;
use crate::session::turn_context::TurnEnvironment;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::handlers::parse_arguments;
use crate::tools::handlers::resolve_tool_environment;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;

const STARTUP_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(250);
const STARTUP_MANIFEST_MAX_BYTES: usize = 1024 * 1024;
const MAX_ALIAS_NAMES: usize = 64;
const MAX_ALIAS_LINE_BYTES: usize = 1024;

include!(concat!(env!("OUT_DIR"), "/trueos_shell3_catalog.rs"));

pub struct HelpHandler {
    include_environment_id: bool,
}

impl HelpHandler {
    pub(crate) fn new(include_environment_id: bool) -> Self {
        Self {
            include_environment_id,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HelpArgs {
    #[serde(default)]
    environment_id: Option<String>,
}

fn help_spec(include_environment_id: bool) -> ToolSpec {
    let mut properties = BTreeMap::new();
    if include_environment_id {
        properties.insert(
            "environment_id".into(),
            JsonSchema::string(Some(
                "Environment id from <environment_context>. Omit for the primary environment."
                    .into(),
            )),
        );
    }
    ToolSpec::Function(ResponsesApiTool {
        name: "help".into(),
        description: "Show local Codex tools and TRUEOS command guidance for the selected local TRUEOS environment. Reads live AKA names from vFile:startup when available; this help catalog does not enumerate installed AppDB names. Use exec_command with cmd=\"cmd help\" for the live installed app catalog. Does not start a shell session or run a process.".into(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(properties, Some(Vec::new()), Some(false.into())),
        output_schema: Some(json!({
            "type":"object",
            "properties":{
                "cwd":{"type":"string"},
                "text":{"type":"string"},
                "catalog_source":{"type":"string"},
                "live_aliases":{"type":"boolean"},
                "appdb_names_available":{"type":"boolean"},
                "shell_session_created":{"type":"boolean"}
            },
            "required":["cwd","text","catalog_source","live_aliases","appdb_names_available","shell_session_created"],
            "additionalProperties":false
        }).into()),
    })
}

fn catalog_source() -> &'static str {
    if SHELL3_GROUPS.is_empty() {
        "unavailable_in_this_build"
    } else {
        "TRUEOS Shell3 registry generated at Codex build time"
    }
}

fn text_error(error: impl std::fmt::Display) -> FunctionCallError {
    FunctionCallError::RespondToModel(error.to_string())
}

fn selected_environment<'a>(
    invocation: &'a ToolInvocation,
    environment_id: Option<&str>,
) -> Result<&'a TurnEnvironment, FunctionCallError> {
    let environment =
        resolve_tool_environment(&invocation.step_context.environments, environment_id)?;
    if environment.environment.is_remote() {
        return Err(text_error(
            "help describes local TRUEOS Shell3; the selected environment is remote",
        ));
    }
    Ok(environment)
}

/// Shared renderer used by the tool and the exec-command help fast path.
/// `cwd` must come from the selected local execution environment.
pub(super) async fn help_text(cwd: &str) -> String {
    let (text, _) = help_document(cwd).await;
    text
}

async fn help_document(cwd: &str) -> (String, bool) {
    let aliases = live_alias_names().await;
    let aliases_available = aliases.is_some();
    (
        render_help(cwd, SHELL3_GROUPS, aliases.as_deref()),
        aliases_available,
    )
}

fn render_help(cwd: &str, groups: &[(&str, &str, &[&str])], aliases: Option<&[String]>) -> String {
    let mut lines = vec![
        String::from("Codex help · local TRUEOS environment"),
        format!("cwd={cwd}"),
        String::from(
            "filesystem=use list_directory to discover paths; file_info for metadata; read_file and write_file for UTF-8 text",
        ),
        String::from(
            "calculator=calculate with no arguments lists the native operation registry; calculate with operation=\"add\" and arguments=[20,22] returns 42 through the synchronous TRUEOS calculator ABI",
        ),
        String::from(
            "spirit=automatically configured at TRUEOS boot, with Lilly as her avatar; /spirit shows integration information; use the spirit tool for native OS interaction, with presentation and input sequencing managed by Spirit: action=windows lists live targets; focus then observe returns a PNG and target grid; pointer/type/key operate the observed target; pointer uses 0..1000 window coordinates while move uses 0..1 hardware-avatar scanout coordinates; emotion selects an expression",
        ),
        String::from(
            "shell=TRUEOS native commands, not POSIX argv or bash; default exec_command uses captured text I/O without terminal repaint sequences; submit exact command names separated by newlines",
        ),
        String::from(
            "captured=use explicit mode-qualified commands such as adm ram or hv status; help, env, ram, smp, net, and gpu also have direct text forms; interactive-only commands require tty=true",
        ),
        String::from(
            "example=exec_command with cmd=\"adm ram\\nhv status\" captures both native diagnostics as one finite batch; its input closes after the batch and any session ID is for polling output",
        ),
        String::from(
            "interactive=tty=true explicitly opens a full Shell3 terminal; fresh terminals start in CMD; Tab cycles HV->CMD->ADM->HV; when a VMX app is selected, the first Tab returns to shell; these sessions accept write_stdin until exit or cancellation",
        ),
        String::from(
            "catalog=group labels describe the interactive Shell3 command registry; captured mode supports text forms and reports unsupported interactive-only requests instead of opening their UI",
        ),
        String::from(
            "session=an empty cmd opens a reusable plain command session for write_stdin; a nonempty captured cmd is a finite batch; neither mode uses POSIX shell syntax",
        ),
        String::from(
            "execution=interactive CMD AppDB names launch in shell context; AKA aliases launch in every interactive mode without Matrix slot selection",
        ),
        String::from("appdb=this local help catalog does not enumerate installed apps; exec_command with cmd=\"cmd help\" reads the live installed AppDB and AKA names"),
    ];

    let mut static_groups = 0usize;
    for (mode, group, names) in groups {
        if group == &"Aka" || group == &"AppDB" || names.is_empty() {
            continue;
        }
        lines.push(format!(
            "mode={mode} group={group} names=[{}]",
            names.join(" ")
        ));
        static_groups += 1;
    }
    if static_groups == 0 {
        lines.push(String::from(
            "shell3_catalog=static command names unavailable in this build",
        ));
    } else {
        lines.insert(
            1,
            String::from("shell3_catalog=static names generated from the TRUEOS Shell3 registry at build time"),
        );
    }

    match aliases {
        Some(aliases) => lines.push(render_aliases(aliases)),
        None => lines.push(String::from(
            "group=Aka scope=all live_names=unavailable (vFile:startup could not be read or parsed)",
        )),
    }
    lines.join("\n")
}

fn render_aliases(aliases: &[String]) -> String {
    let prefix = "group=Aka scope=all live_names=[";
    let mut line = String::from(prefix);
    let mut emitted = 0usize;
    let mut omitted = 0usize;
    for name in aliases {
        if name.chars().any(char::is_control) {
            omitted += 1;
            continue;
        }
        let quoted = serde_json::to_string(name).unwrap_or_else(|_| String::from("\"<invalid>\""));
        let separator_len = usize::from(emitted != 0);
        if emitted >= MAX_ALIAS_NAMES
            || line.len() + separator_len + quoted.len() + 32 > MAX_ALIAS_LINE_BYTES
        {
            omitted += 1;
            continue;
        }
        if emitted != 0 {
            line.push(',');
        }
        line.push_str(&quoted);
        emitted += 1;
    }
    line.push(']');
    if omitted != 0 {
        line.push_str(&format!(" omitted={omitted}"));
    }
    line
}

#[cfg(target_os = "trueos")]
async fn live_alias_names() -> Option<Vec<String>> {
    #[derive(Deserialize)]
    struct StartupConfig {
        #[serde(default)]
        aliases: Vec<StartupAlias>,
    }
    #[derive(Deserialize)]
    struct StartupAlias {
        name: String,
    }

    let bytes = tokio::time::timeout(
        STARTUP_READ_TIMEOUT,
        trueos_v::vfs_async::read_file(b"vFile:startup"),
    )
    .await
    .ok()?
    .ok()?;
    if bytes.len() > STARTUP_MANIFEST_MAX_BYTES {
        return None;
    }
    let config: StartupConfig = serde_json::from_slice(&bytes).ok()?;
    Some(config.aliases.into_iter().map(|alias| alias.name).collect())
}

#[cfg(not(target_os = "trueos"))]
async fn live_alias_names() -> Option<Vec<String>> {
    None
}

impl ToolExecutor<ToolInvocation> for HelpHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain("help")
    }

    fn spec(&self) -> ToolSpec {
        help_spec(self.include_environment_id)
    }

    fn supports_parallel_tool_calls(&self) -> bool {
        true
    }

    fn handle<'a>(&'a self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'a>
    where
        ToolInvocation: 'a,
    {
        Box::pin(async move {
            let ToolPayload::Function { arguments } = &invocation.payload else {
                return Err(text_error("help requires function arguments"));
            };
            let args: HelpArgs = parse_arguments(arguments)?;
            let environment = selected_environment(&invocation, args.environment_id.as_deref())?;
            let cwd = environment.cwd().inferred_native_path_string();
            let (text, live_aliases) = help_document(&cwd).await;
            Ok(boxed_tool_output(JsonToolOutput::new(json!({
                "cwd": cwd,
                "text": text,
                "catalog_source": catalog_source(),
                "live_aliases": live_aliases,
                "appdb_names_available": false,
                "shell_session_created": false
            }))))
        })
    }
}

impl CoreToolRuntime for HelpHandler {
    fn is_builtin_control_tool(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_explains_native_syntax_and_app_capabilities() {
        let groups = &[
            ("HV", "", &["status", "peer"][..]),
            ("ADM", "System", &["net", "disc"][..]),
            ("VMX", "VME", &["stop", "tui"][..]),
        ];
        let text = render_help("/apps/codex", groups, Some(&["weather".into()]));
        for expected in [
            "cwd=/apps/codex",
            "calculate with no arguments lists the native operation registry",
            "not POSIX argv or bash",
            "HV->CMD->ADM->HV",
            "fresh terminals start in CMD",
            "cmd=\"adm ram\\nhv status\"",
            "default exec_command uses captured text I/O",
            "interactive-only commands require tty=true",
            "an empty cmd opens a reusable plain command session",
            "group labels describe the interactive Shell3 command registry",
            "mode=ADM group=System names=[net disc]",
            "group=Aka scope=all live_names=[\"weather\"]",
            "AppDB",
            "this local help catalog does not enumerate installed apps",
            "cmd=\"cmd help\" reads the live installed AppDB and AKA names",
        ] {
            assert!(text.contains(expected), "missing {expected:?} in {text}");
        }
    }

    #[test]
    fn aliases_are_escaped_bounded_and_explicitly_truncated() {
        let aliases: Vec<String> = (0..90)
            .map(|index| format!("alias-{index:03}-{}", "x".repeat(20)))
            .collect();
        let line = render_aliases(&aliases);
        assert!(line.len() <= MAX_ALIAS_LINE_BYTES);
        assert!(line.contains("omitted="));
        assert!(line.starts_with("group=Aka scope=all live_names=[\"alias-000-"));

        let escaped = render_aliases(&["with\"quote".into(), "with\\slash".into()]);
        assert!(escaped.contains("\"with\\\"quote\""));
        assert!(escaped.contains("\"with\\\\slash\""));
    }

    #[test]
    fn missing_live_sources_are_not_reported_as_empty_alias_catalogs() {
        let text = render_help("/apps/codex", &[], None);
        assert!(text.contains("static command names unavailable"));
        assert!(text.contains("live_names=unavailable"));
    }

    #[test]
    fn catalog_source_matches_embedded_catalog_availability() {
        if SHELL3_GROUPS.is_empty() {
            assert_eq!(catalog_source(), "unavailable_in_this_build");
        } else {
            assert_eq!(
                catalog_source(),
                "TRUEOS Shell3 registry generated at Codex build time"
            );
        }
    }
}
