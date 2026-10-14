# Shell command support

The `nopowersh` feature disables PowerShell parsing, command unwrapping,
UTF-8 prefix injection, and executable discovery. Compatibility APIs return
`None` (or pass the invocation through), so unsupported scripts stay opaque.
Shell names and enum variants remain available for protocol compatibility.

`codex-cli/trueos-blueprint` enables `nopowersh` automatically for sh3.
The PowerShell C grammar dependency is excluded by `target_os = "trueos"`;
TRUEOS builds without `nopowersh` fail explicitly. Ordinary host builds retain
PowerShell support. Enabling `nopowersh` on a host disables interpretation but
still resolves the grammar dependency: Cargo features cannot subtract dependencies.

Bash parsing and the shared Tree-sitter runtime remain enabled. This feature
does not remove Oniguruma, TLS cryptography, compression, or BLAKE3.
