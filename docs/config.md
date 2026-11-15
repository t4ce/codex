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

The standard TRUEOS Blueprint disables XERJ. No XERJ process, engine or
Blueprint download is requested by the default Codex launch. Its disabled
entry may remain visible in configuration/status views; existing installations
are retained.

Optional opt-in belongs in the user's `config.toml`:

```toml
[mcp_servers.xerj]
enabled = true
```

When explicitly enabled and `xerj.bp` is absent from AppDB, the native MCP
launcher asks Apps to install `xerj-service` before starting its MCP child.
The engine stays lazy until a tool needs it. Custom commands and explicit
executable paths retain their normal launch behavior.

## TRUEOS local read-aloud

The standard TRUEOS Blueprint enables read-aloud automatically. No launch
arguments or launch script are required. A user configuration can disable it:

```toml
[tui]
trueos_tts = false
```

This preference ignores project configuration. The native TUI warms the
on-demand `ttstt` Blueprint at startup and reuses an existing healthy service.
Completed public replies use its loopback speech service. Reasoning, tools, history replay and realtime
delegated voice replies are excluded. Hidden display directives, code blocks
and link destinations are omitted; Kokoro's English G2P frontend owns
pronunciation. No plugin, MCP server, microphone or cloud realtime session is
required. This setting currently applies to the TRUEOS Rust TUI, not the
VS Code extension or noninteractive `codex exec`.

ttstt ships the Kokoro model, English G2P and Heart (`af_heart`) voice inside
the Blueprint. It needs no setup, model directory, downloads or Whisper.
This integration bundle supports English and Heart only. Startup and speech requests run away from the
render loop; failures preserve the displayed reply. Pending answers may be
cleared or replaced, but already playing speech runs to completion. See
`TRUEOS-Blueprints/apps/ttstt/README.md` for host WAV verification and
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
