# Codex CLI

[**Codex CLI Documentation**](https://developers.openai.com/codex/cli)

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
