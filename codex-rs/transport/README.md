# codex-transport

Codex API, generic retry/SSE policy, HTTP execution, and WebSocket connection setup share one Cargo compilation unit. Their source boundaries remain public Rust modules:

| Module | Responsibility |
| --- | --- |
| `codex_transport::api` | Typed Responses, memory, model, image, search, and realtime APIs; provider/auth/header settings; API error mapping |
| `codex_transport::client` | `RetryPolicy`, `RetryOn`, `run_with_retry`, `backoff`, raw SSE framing, and request telemetry |
| `codex_transport::http` | HTTP request/response handling, proxy selection, pooled routes, CA configuration, cookies, and transport diagnostics |
| `codex_transport::websocket` | Proxy-aware WebSocket dialing, TLS handshakes, shared cookies, and authorization throughout connection use |

`codex-network-foundation` owns the shared error identities, retry deadlines, destination policy and revocation, and RustCrypto provider. Protocol data and the managed proxy can use that lower layer without depending on API or transport execution. This resolves both feedback paths created by folding the API and HTTP packages: protocol errors referring to HTTP errors, and the managed proxy initializing the TLS provider. WebSocket execution is folded here too because its connector consumes the HTTP factory.

The API module hosts the request/response models and request builders for Responses and related APIs. It owns provider configuration (base URLs, headers, query parameters), auth header injection, retry tuning, and stream idle settings. It parses SSE streams into `ResponseEvent`/`ResponseStream`, including rate-limit snapshots and API-specific error mapping. Higher layers retain auth refresh and business logic.

## Core API interface

Responses callers provide `ResponsesApiRequest` for the model, input, tools, parallel tool calls, reasoning, and text controls. `ResponsesOptions` carries transport/header concerns such as conversation/session identity, extra headers, compression, and turn state. The result is a `ResponseStream` of `ResponseEvent`.

Memory summarize callers provide `MemorySummarizeInput`: a model, optional reasoning, and `Vec<RawMemory>` serialized as `traces`. Each raw memory retains its ID, source-path metadata, and normalized items. `MemoriesClient::summarize_input` wraps JSON encoding, retries, and telemetry, returning `Vec<MemorySummarizeOutput>`.

Original unit and integration coverage remains part of this package, including CA subprocess fixtures, HTTP routing and revocation, API SSE/realtime behavior, and WebSocket proxy/cookie tests. `custom_ca_probe` retains its binary target name for subprocess tests.
