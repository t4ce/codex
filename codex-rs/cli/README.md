# Platform-owned installation

The CLI and TUI expose an `install-update` Cargo feature, enabled by default for
normal desktop builds. It includes the managed app-server daemon, package
copying and release selection, background/manual updates, CLI self-update, and
desktop installer entry points.

TRUEOS Blueprint packing already selects `--no-default-features --features
trueos-blueprint`, which omits `install-update`. TRUEOS owns pack delivery and
AppDB installation; Codex uses its embedded app-server and does not create a
second managed installation or launch an updater. Login, configuration,
onboarding, and other first-run application setup remain available. Explicit
connections to an existing remote app-server and `app-server --stdio` remain
available. The daemon management and daemon-based `remote-control` CLI commands
are omitted; the app-server itself still supports remote control.

For a host build with the same installation policy:

```sh
cargo build -p codex-cli --no-default-features
```

`codex-app-server-daemon` is absent from that application's dependency graph.
BLAKE3 remains a dependency of skill state tracking; omitting installation
management does not remove all uses of BLAKE3.
