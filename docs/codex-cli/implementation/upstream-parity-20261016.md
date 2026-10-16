# Upstream parity integration — 2026-10-16

This integration merges all 195 upstream commits into the TRUEOS fork while retaining its 30 local commits. The initial merge produced 58 conflicted paths.

| Input | Exact commit |
| --- | --- |
| Local HEAD | `7a19e0a9d8ffcf8c6e61d1f6eb0b16fd10f17e90` |
| Upstream | `0e67502123b4c6952b9fa5d672d2907fe4059f56` |
| Merge base | `a9b0815167c27ab13719dd12d13b63e02b70d221` |

## Commit scope

The per-commit audit is recorded in [`upstream-parity-20261016-commits.json`](upstream-parity-20261016-commits.json), including subjects, paths, short keywords, and mixed test/runtime flags. Runtime commits that also change tests remain runtime changes; inline tests can occur within runtime source files.

| Group | Commits |
| --- | ---: |
| common-runtime | 160 |
| docs/tests-only | 15 |
| CI | 7 |
| releases | 2 |
| Windows-macOS-only | 11 |
| AWS-specific | 0 |

The ten oldest upstream commits, in integration order:

| Commit | Top three keywords | Group |
| --- | --- | --- |
| `7e9c061ceb` | remote execution, stream closure, cleanup | common-runtime |
| `6e5f665feb` | remote polling, inherited streams, regression test | docs/tests-only |
| `ddb33e34b2` | test removal, protocol/MCP, apply-patch/TUI | docs/tests-only |
| `6c372fbef4` | message board, availability, turn admission | common-runtime |
| `0c7362d121` | unsafe-libyaml, advisory exception, dependency audit | CI |
| `27e6192e84` | Guardian Decisions, credential cache, authentication | common-runtime |
| `cab328f275` | Guardian, parent compaction, failure reasons | common-runtime |
| `97239fd052` | local board, explicit connections, runtime access | common-runtime |
| `16845bb8e8` | arg0 helpers, OS isolation, temporary directories | common-runtime |
| `bf4ca52b61` | thread permissions, configuration loading, restore | common-runtime |

## Port invariants and merge decisions

- Existing TRUEOS target gates and native filesystem abstractions remain in place. File metadata accepts native Tokio metadata on TRUEOS alongside standard host metadata. Upstream streaming file replacement and large direct WebSocket writes remain available.
- Consolidated transport and utility crates retain their local module layout. Imports from removed upstream crates map through `transport-migration.json` and `utility-path-migration.json`; shared error identities remain in network foundation.
- Executor capability discovery V2, skill/plugin metadata, credential masking, executor credentials, and broker-aware proxy preparation are retained. Production discovery resolves and caches fixed executor-global Codex, user, and system skill locations on first initialization. In-process discovery tests seed the cache with fixture paths; connection-only test constructors skip startup prewarming. The capability subprocess test supplies HOME, USERPROFILE, and CODEX_HOME through its child environment. The generic subprocess harness isolates CODEX_HOME only and can inherit user/system skill roots.
- Unified execution tracks `stdin_open` independently of TTY and output-drain policy. Native piped stdin remains writable; remote stdin follows the actual launch configuration. Closed stdin permits interrupt handling and rejects ordinary input. Writes remain serialized with process interactions and revalidate process identity after approval.
- Upstream remote exit semantics retain inherited output streams until closure and collection. A write observing remote exit proceeds to collect final output. Write errors check process exit without removing the process before collection; final removal uses `UntilCollected`, replacing the earlier cached `status_after_write` path. Fatal errors on live processes still terminate and release the handle.
- Session phase timings remain around tool/context preparation and checkpoint persistence. V2 discovery adds its own timed phase, and context recording returns the upstream `Arc<WorldState>` on every success path. Incomplete-response spending accounting is retained.
- SQLite existence checks use the native service namespace on TRUEOS. Initialization adopts upstream `connect_once`, busy retry, failed-pool cleanup, preserved journal modes, and cached NFS detection. Incremental auto-vacuum initializes only an empty schema in mode NONE, preserving existing FULL/incremental modes. The WAL setter is excluded on TRUEOS, where journal ownership belongs to the service.

## Validation

Validation is pending. Source resolution and formatting are integration preparation, not evidence that builds or tests pass. Cargo dependency/lock reconciliation, focused host suites, TRUEOS target compilation and the repository-required integration checks must be completed and recorded before the parity PR is considered validated. No test success is claimed by this report.
