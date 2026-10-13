# TRUEOS socket paths in Codex

Source marker: `TRUEOS_SOCKET_PATH`. Find the integration points with:

```sh
rg -n TRUEOS_SOCKET_PATH codex-rs
```

This is a source inventory. The markers do not create TRUEOSFS node kinds,
access-key metadata, or a vfile capability. A pathname socket is a socket
endpoint addressed through a filesystem namespace; it is not a regular file
containing network data. TRUEOSFS currently has files and directories, so these
assumptions need an explicit transport mapping before they can run on TRUEOS.

## App-server control endpoint

| Artifact | Actual role | Source |
| --- | --- | --- |
| `/tmp/codex-daemon-<uid>/<sha256>` on Unix, after resolving `/tmp` | Physical socket node carrying WebSocket JSON-RPC | `app-server-transport/src/transport/unix_socket.rs`, `uds/src/daemon_directory.rs` |
| `$CODEX_HOME/app-server-control/app-server-control.sock`, or a custom advertised path | Unix rendezvous symlink pointing to the physical socket | `app-server-transport/src/transport/unix_socket.rs` |
| Physical socket path with `.lock` extension | Regular startup-lock file | `app-server-transport/src/transport/unix_socket.rs` |
| `$CODEX_HOME/app-server-control/app-server-startup.lock` | Regular app-server startup-lock file | `app-server-transport/src/transport/mod.rs` |
| Parent directories | Ordinary directory nodes containing these entries | `uds/src/daemon_directory.rs`, `app-server-transport/src/transport/unix_socket.rs` |

The physical name hashes the canonical advertised path. Socket cleanup unlinks
the socket node and its owned rendezvous alias. `ControlSocketPathGuard` owns
that cleanup; `AppServerStartupLock` owns the regular lock file. Keep the socket,
alias, lock, and directory roles distinct in future filesystem integration.

## Other integration points

Paths below `codex-rs/`:

| Marker | Source | Role |
| --- | --- | --- |
| `uds` | `uds/src/lib.rs` | Shared pathname socket bind/connect boundary |
| `remote-client` | `app-server-client/src/remote.rs` | WebSocket client connecting to the control socket |
| `daemon-client` | `app-server-daemon/src/client.rs` | Daemon control client connecting to the control socket |
| `updater-listener` | `app-server-daemon/src/update_loop.rs` | Separate manual-update request socket |
| `updater-client` | `app-server-daemon/src/manual_update.rs` | Connects to the updater socket |
| `ide-context` | `tui/src/ide_context/ipc.rs` | IDE context IPC; primary `$CODEX_HOME/ipc/ipc.sock` and legacy socket paths |
| `stdio-relay` | `stdio-to-uds/src/lib.rs` | Relays stdin/stdout over an existing socket endpoint |

The app-server already has `stdio://` and TCP WebSocket `ws://IP:PORT`
transports. A future TRUEOS vfile capability could resolve a filesystem-visible
socket address to an owned network endpoint. That mapping remains deferred;
the tags identify the boundaries to revisit and do not provide that capability.
