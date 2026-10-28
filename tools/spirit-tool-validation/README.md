# Native Spirit tool checks

From the repository root:

```sh
tools/spirit-tool-validation/run.sh --locked
```

The runner also works when invoked by an absolute path from another directory.

The fixture imports the production Codex Spirit handler, JSON schema types and
the public `trueos::spirit` wrappers from the sibling `TRUEOS-Blueprints` checkout.
It supplies a small Codex runtime fixture and deterministic native ABI functions;
no test contacts a live cursor, keyboard, window service or rig.

Checks cover all eight native actions, coordinate spaces, Unicode input limits,
strict per-action fields, schema registration, image output and safe logging,
local-environment/image-model guards, and exactly-once dispatch when native input
returns Busy or transport errors. The shared schema's own test runs too.

The same fixture imports the production native agent snapshot projection. It
checks canonical nicknames from live session registries and startup sources,
unloaded V2 children, and parent paths scoped to separate agent trees. The small
runtime inputs do not spawn agents or require a live model connection.

This standalone workspace avoids the full Codex host dependency graph. A native
Codex package build still verifies the handler against the complete runtime.
The runner defaults to `nightly-2026-07-10`; set `CODEX_SPIRIT_TEST_TOOLCHAIN` to
choose another installed toolchain. Cargo arguments such as `--offline` and
`--jobs 2` pass through to the test command.
