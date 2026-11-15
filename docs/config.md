# Configuration

For basic configuration instructions, see [this documentation](https://developers.openai.com/codex/config-basic).

For advanced configuration instructions, see [this documentation](https://developers.openai.com/codex/config-advanced).

For a full configuration reference, see [this documentation](https://developers.openai.com/codex/config-reference).

## Next-prompt suggestions

After a live response completes, the interactive CLI can suggest a next user message as dim text
in an empty composer. Press **Tab** to accept it into the editable draft, then **Enter** to send.
Press **Esc** or type your own message to dismiss it. Enter alone never sends a suggestion.
Drafts, queued messages, replayed history, and active goals take precedence. Suggestions may be
omitted when there is no useful next step or generation fails.

The feature uses a bounded, tool-free background model request with recent conversation text.
It uses the same implementation on Ubuntu/Linux and TRUEOS. It is enabled by default; disable
it with `codex --disable next_prompt_suggestions` or in `config.toml`:

```toml
[features]
next_prompt_suggestions = false
```

## Optional XERJ on TRUEOS

Disable XERJ for a session with:

```sh
codex -c mcp_servers.xerj.enabled=false
```

Or persist the setting in your Codex home `config.toml`:

```toml
[mcp_servers.xerj]
enabled = false
```

This removes XERJ from MCP startup and tool discovery: no XERJ child, engine,
or Blueprint download is requested by Codex. The disabled entry may still appear
in configuration and MCP status views. Existing installations are retained.

XERJ remains enabled by default. When enabled and its native `xerj.bp` executable
is absent from AppDB, the local MCP launcher asks Apps to install `xerj-service`
before starting the MCP child. The engine stays lazy until a tool needs it.
Custom replacement commands and explicit executable paths keep their normal
launch behavior. Re-enable with `-c mcp_servers.xerj.enabled=true`.

## TRUEOS local read-aloud

Opt in with `codex -c tui.trueos_tts=true`, or add to your user config:

```toml
[tui]
trueos_tts = true
```

The default is off. This preference ignores project configuration. Completed
public replies in the active TUI launch the on-demand `ttstt` Blueprint and
use its loopback speech service. Reasoning, tools, history replay and realtime
delegated voice replies are excluded. Hidden display directives, code blocks
and link destinations are omitted; Kokoro's lexicon and G2P frontend own
pronunciation. No plugin, MCP server, microphone or cloud realtime session is
required. This setting currently applies to the TRUEOS Rust TUI, not the
VS Code extension or noninteractive `codex exec`.

ttstt requires the existing Kokoro native model assets under `/models/kokoro`.
It does not require Whisper. Startup and speech requests run away from the
render loop; failures preserve the displayed reply. Pending answers may be
cleared or replaced, but already playing speech runs to completion. See
`TRUEOS-Blueprints/apps/ttstt/README.md` for assets, host WAV verification and
the local service contract.

Upstream already provides `thread/realtime/appendSpeech`, but it is tied to
an active cloud realtime conversation ([upstream implementation, checked
2026-11-15](https://github.com/openai/codex/blob/3b8bd2ee7e95b058192151b3468093a5a67655f8/codex-rs/core/src/realtime_conversation.rs#L1289)).
It is not a local speech provider API; this integration instead selects the
public final at turn completion.

## Lifecycle hooks

Admins can set top-level `allow_managed_hooks_only = true` in
`requirements.toml` to ignore user, project, and session hook configs while
still allowing managed hooks from requirements and managed config layers. This
setting is only supported in `requirements.toml`; putting it in `config.toml`
does not enable managed-hooks-only mode.
