# Codex CLI

[**Codex CLI Documentation**](https://developers.openai.com/codex/cli)

## Clipboard backend

The `system-clipboard` Cargo feature is enabled by default. To compile out native
clipboard access and terminal/subprocess clipboard forwarding, run:

```sh
cargo run --bin codex --no-default-features -- --no-daemon
```

Copy/paste handlers, selections, and the clipboard worker remain available, but
clipboard operations report that the backend is unavailable. Text paste delivered
by the terminal still works. The disabled copy backend lives in
`tui/src/clipboard_copy/disabled.rs`; clipboard text and image reads retain their
entry points in `tui/src/clipboard_paste/`. These are the replacement points for a
future platform clipboard API such as TRUEOS.

To explicitly enable clipboard support with defaults disabled, add
`--features system-clipboard` before `--`.

## TRUEOS execution defaults

When TRUEOS provides application isolation through VMX, set these user defaults
in `~/.codex/config.toml`:

```toml
sandbox_mode = "danger-full-access"
approval_policy = "never"
```

Then start the terminal interface without native clipboard support or the shared
background server:

```sh
cargo run --bin codex --no-default-features -- --no-daemon
```

These settings disable Codex-managed command sandboxing and approval prompts;
TRUEOS owns isolation and external access. Startup skips the bubblewrap probe
for this policy. The native sandbox code remains compiled into the binary.
The defaults apply to other Codex sessions using the same user config unless
overridden. The exact `externalSandbox` policy is available through app-server
requests, rather than the CLI sandbox setting.

## Optional integrations in this checkout

This checkout disables these integrations by default. ChatGPT and OpenAI API
authentication remain available.

| Cargo feature | Enables |
| --- | --- |
| `bedrock` | Amazon Bedrock and AWS authentication |
| `local-models` | Ollama and LM Studio setup/model management |
| `voice` | Voice transport and audio processing |
| `external-agent-imports` | Import settings and history from other assistants |
| `external-reporting` | Analytics, OTLP export, and feedback uploads |

```sh
cargo run --locked -p codex-cli --bin codex -- --no-daemon
```

To include an integration, add its Cargo feature before `--`, for example
`--features local-models,voice`.
Disabled builds reject Bedrock login/setup and AWS request signing with an explicit
error rather than falling back to OpenAI credentials.
Local-model setup and new audio/voice requests likewise report that their feature
is disabled. Text conversations and existing ChatGPT credentials remain supported.
