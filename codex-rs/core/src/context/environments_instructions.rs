use codex_protocol::protocol::ENVIRONMENTS_INSTRUCTIONS_CLOSE_TAG;
use codex_protocol::protocol::ENVIRONMENTS_INSTRUCTIONS_OPEN_TAG;

use super::ContextualUserFragment;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ContentItemKind;
use codex_protocol::models::input_text;

pub(crate) struct EnvironmentsInstructions;

#[cfg(target_os = "trueos")]
pub(super) const TRUEOS_GUIDANCE_HEADING: &str = "## Local TRUEOS tools";

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

    fn body(&self) -> ContentItem {
        let body = "\n## Execution environments\n\
Execution environments are separate machines or workspaces with their own files, shell, and installed capabilities. `<environment_context>` lists the environments selected for this task.\n\
\n\
An environment marked `starting` is not yet usable. Its files, commands, AGENTS.md instructions, skills, plugins, and MCP tools may become available when startup completes.\n\
\n\
Wait only when the current task needs that environment. Continue using tools that are already available for unrelated work.\n"
            .to_string();
        #[cfg(target_os = "trueos")]
        let body = format!(
            "{body}\n{TRUEOS_GUIDANCE_HEADING}\n\
- Open app or desktop task: call `spirit` with `action=windows` first, then `action=observe` with the target `window_id`. This focuses the shared UI4 window and returns a PNG and its coordinate grid in one call. Use `pointer`, `type`, and `key` to perform the requested UI work, then observe to verify it. A request for a UI outcome is enough to choose and carry out routine input steps. `pointer` uses the window's 0..1000 grid. `move` moves Lilly herself in 0..1 scanout coordinates. `filmstrip` samples a window over time.\n\
- `windows` lists visible frames shared with Spirit. If the requested open window is missing, tell the user to right-drag that window onto the green circle around Lilly and release, then check again. Shared windows show an AI badge. A Document Control or app MCP inventory only describes its own integration. The native UI path works independently.\n\
- Use collaboration tools for independent work when that helps complete the task. A simple cursor move or a short UI sequence can be done directly. Lilly's UI4 window selection, pointer, and keyboard are shared, so designate one agent to drive them while other agents handle independent work.\n\
- For ordinary files use `list_directory`, `file_info`, `read_file`, `write_file`, and `copy_file`. With no path, `list_directory` starts in the environment cwd. Use the returned paths and the environment's own filesystem permissions. Host checkout paths may be absent. Use `calculate` for small local computations.\n\
- For a native command or app launch, call `help` for the live AppDB and Shell3 groups. Local `exec_command` sends keyboard input to Shell3 under the initiating user's permissions. Send an exact AppDB name in CMD, `§§name` followed by Enter for an online app, or Tab to change mode (CMD → ADM → HV → CMD). Group names are labels. Use `write_stdin` for follow-up input until exit or cancellation. The local command accepts Shell3 input without Unix argv, shell, login, or workdir options. Remote environments use their reported shell and filesystem.\n"
        );
        input_text(body)
    }
}
