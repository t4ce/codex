# codex-network-foundation

Shared network contracts used by protocol errors, transport implementations, and managed proxy TLS setup. This package owns the original concrete HTTP errors and their source chains, monotonic retry deadlines, destination authorization and permission revocation, and process-wide RustCrypto provider selection.

The dependency direction stays below protocol and transport: this crate has no dependency on `codex-protocol`, `codex-transport`, or `codex-network-proxy`. HTTP requests, proxy resolution, socket dialing, CA file loading, and API processing remain in their execution layers.

Provider tests remain separate integration executables so installing a provider in one test cannot affect the other. The original revocation and retry-deadline tests execute against the canonical definitions here.
