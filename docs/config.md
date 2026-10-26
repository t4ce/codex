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

## Lifecycle hooks

Admins can set top-level `allow_managed_hooks_only = true` in
`requirements.toml` to ignore user, project, and session hook configs while
still allowing managed hooks from requirements and managed config layers. This
setting is only supported in `requirements.toml`; putting it in `config.toml`
does not enable managed-hooks-only mode.
