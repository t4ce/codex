#!/usr/bin/env bash
set -euo pipefail

fixture_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
exec cargo "+${CODEX_SPIRIT_TEST_TOOLCHAIN:-nightly-2026-07-10}" test \
  --manifest-path "$fixture_dir/Cargo.toml" \
  --target x86_64-unknown-linux-gnu "$@"
