# TRUEOS Lily response client checks

This small host fixture compiles the production `tui/src/trueos_lily.rs` without
building the full Codex TUI. Its localhost HTTP server exercises protocol/backend
readiness, latest-only transport, view-drop cancellation and final clear while
reconnecting. Pure tests cover verbatim Unicode bounds, cadence and the ten-second
pending-text expiry. No test executes VNNI or accesses Spirit; those belong to the
separate Lily Blueprint VM.

```sh
cargo test --manifest-path codex-rs/tui/tests/trueos_lily/Cargo.toml
cargo clippy --manifest-path codex-rs/tui/tests/trueos_lily/Cargo.toml --all-targets -- -D warnings
```

Native launch follows the SQLite service pattern: probe `127.0.0.1:9771/health`,
request Apps with `launch --headless lily2vec\n` if needed, then await readiness
in the independent network worker. Codex only retains a bounded text snapshot in
a watch slot. The worker POSTs `{source, sequence, text, clear}` to `/v1/snapshots`;
clear events carry a higher sequence and remain deliverable after text expires.
Neither launcher nor HTTP waits run in a TUI response handler. The independent
service is not terminated when Codex closes.
