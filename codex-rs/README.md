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

## Local MCP servers on TRUEOS

The TRUEOS local MCP launcher starts the configured Blueprint in a fresh VMX
Hull through the hypervisor child API. A bare `command` names an app.db archive
(with `.bp` appended when omitted); a path selects a Blueprint archive through
the caller's filesystem access. Arguments and environment values are UTF-8.
The working directory is resolved within the caller's filesystem scope.
Kernel-owned `TRUEOS_*` environment values are not inherited from the request.

The server uses ordinary stdin/stdout for newline-delimited MCP JSON. The kernel
routes those bytes through the generation-bound child channel; stderr and runtime
logs remain console output. Messages are streamed across bounded communication
pages with backpressure. Closing or dropping the client terminates its owned VM.
The launch metadata must fit one hypervisor communication page. Executables must
be TRUEOS Blueprints; this does not introduce Unix executable emulation.

This requires a kernel with `trueos_cabi_blueprint_child_spawn_program_v1` and
`trueos_cabi_blueprint_child_read_v1`, plus the matching Blueprint SDK. Rebuilding
Codex alone does not update the running kernel. Linux/macOS/Windows retain their
existing local process launcher; HTTP and remote-executor MCP paths are unchanged.

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
