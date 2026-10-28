use codex_protocol::protocol::ENVIRONMENTS_INSTRUCTIONS_CLOSE_TAG;
use codex_protocol::protocol::ENVIRONMENTS_INSTRUCTIONS_OPEN_TAG;

use super::ContextualUserFragment;
use codex_protocol::models::ContentItemKind;

pub(crate) struct EnvironmentsInstructions;

impl ContextualUserFragment for EnvironmentsInstructions {
    fn content_kind(&self) -> ContentItemKind {
        ContentItemKind("environments.instructions".to_string())
    }

    fn role(&self) -> &'static str {
        "developer"
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        (
            ENVIRONMENTS_INSTRUCTIONS_OPEN_TAG,
            ENVIRONMENTS_INSTRUCTIONS_CLOSE_TAG,
        )
    }

    fn body(&self) -> String {
        let body = "\n## Execution environments\n\
Execution environments are separate machines or workspaces with their own files, shell, and installed capabilities. `<environment_context>` lists the environments selected for this task.\n\
\n\
An environment marked `starting` is not yet usable. Its files, commands, AGENTS.md instructions, skills, plugins, and MCP tools may become available when startup completes.\n\
\n\
Wait only when the current task needs that environment. Continue using tools that are already available for unrelated work.\n"
            .to_string();
        #[cfg(target_os = "trueos")]
        let body = format!(
            "{body}\nThe local runtime is TRUEOS. Call `help` for local capabilities and native command groups before opening a command session; Codex answers it locally. For small numeric computations, use `calculate` to call TRUEOS's synchronous calculator ABI directly; call it with no arguments to discover operations. For filesystem discovery, call `list_directory` with no path to start at the environment cwd, then use the returned paths with `file_info`, `read_file`, and `write_file`. These tools use the selected environment's filesystem and permissions. Build-host checkout paths are not automatically present in this runtime. Local `exec_command` sends native interactive Shell3 input; use the direct filesystem tools for file operations. Remote environments retain their reported filesystem and shell semantics.\n"
        );
        body
    }
}
