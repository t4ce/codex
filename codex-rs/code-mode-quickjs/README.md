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

The `trueos-qjs/native-engine` dependency must supply engine-only C bindings and
header-defined value ownership wrappers. Its application-wide `trueos` feature
must not be enabled: that feature includes the qjs application's independent
allocator/time compatibility symbols.

Host validation uses the same QuickJS source used by native packaging:

```sh
TRUEOS_QJS_QUICKJS_DIR=/path/to/pinned/quickjs cargo test -p codex-code-mode-quickjs --lib
```
