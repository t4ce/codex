# Native QuickJS Code Mode

This provider executes actual JavaScript ES modules using TRUEOS's existing
QuickJS C engine. It implements `CodeModeSessionProvider`; no subprocess,
remote execution endpoint, Unix shell or V8 JIT is involved. The existing Codex
delegate remains the only authority for nested tool calls.

Each cell has its own interpreter context and module scope. Session `store` and
`load` values persist between cells; lexical declarations remain cell-local,
as in the existing V8 backend. Supported APIs include `tools`, `ALL_TOOLS`,
`text`, `store`, `load`, `notify`, timers, `as_settled`, `stream_settled`,
`yield_control`, and `exit`. Tool names are normalized from the enabled catalog;
callbacks retain the original `ToolName` and function/freeform kind.

Cells support observation yields, waiting, preemption and termination. A
QuickJS interrupt hook checks cancellation and periodically yields the native
thread; promise job pumping is also bounded between cooperative yields. Heap
and captured text output are bounded. Interpreter values never cross threads.
Shutdown cancels pending delegates/timers and joins their async tasks before
reporting a cell closed.

Image/audio helpers currently raise explicit unsupported errors. External
module loading is unavailable. This is an interpreter foundation; it does not
claim V8 API parity or a separate memory protection boundary within Codex.

Code Mode uses `hull-quickjs` with its `raw` feature from
`TRUEOS-Buildins/hull/crates/quickjs-engine`. Hull owns the pinned C sources,
full-pointer value ABI and header-defined ownership wrappers. Code Mode owns
its contexts, cancellation and tool delegate; browser globals are not installed.

Host validation uses Hull's vendored engine without a network checkout:

```sh
cargo test -p codex-code-mode-quickjs --lib
```

Browser actions on TRUEOS open Hull. For HTML projects, the `trueos_ide` tool
opens an Edit workspace with `kind: "html"`; `run` saves the project, starts
Edit's preview server and opens its validated loopback URL in Hull. Native
projects continue to use Edit's compile/run lifecycle.
