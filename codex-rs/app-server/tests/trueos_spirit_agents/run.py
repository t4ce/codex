#!/usr/bin/env python3
"""Run the native observer against real projection/watch/wire code on the build host.

The normal host workspace cannot resolve the TRUEOS Hyper std-compat overlay.
Only kernel ABI calls and agent-runtime storage are mocked here. Native cfgs are
enabled in the copied watch module; its unrelated existing tests are omitted.
"""

import os
from pathlib import Path
import subprocess
import tempfile


HERE = Path(__file__).resolve().parent
CODEX_RS = HERE.parents[2]
REPOSITORIES = CODEX_RS.parent.parent
fixture = Path(tempfile.mkdtemp(prefix="trueos-spirit-agent-producer-"))
(fixture / "src").mkdir()
watch = (CODEX_RS / "app-server/src/thread_status.rs").read_text()
watch = watch.split("\n#[cfg(test)]\nmod tests {", 1)[0]
watch = watch.replace('#[cfg(target_os = "trueos")]\n', "")
watch = watch.replace("#[cfg(test)]\n", "")
(fixture / "src/watch.rs").write_text(watch)
source = (HERE / "fixture.rs").read_text()
source = source.replace("@CODEX_RS@", str(CODEX_RS))
source = source.replace("@REPOSITORIES@", str(REPOSITORIES))
(fixture / "src/lib.rs").write_text(source)
(fixture / "Cargo.toml").write_text('''[package]
name = "trueos-spirit-agent-producer-fixture"
version = "0.0.0"
edition = "2024"

[dependencies]
tokio = { version = "=1.54.1", features = ["rt", "macros", "sync", "time"] }
serde = { version = "=1.0.228", features = ["derive"] }
serde_json = "=1.0.150"
tracing = "=0.1.44"
''')
print(f"Native Spirit producer fixture: {fixture}", flush=True)
env = dict(os.environ)
env.setdefault("CARGO_TARGET_DIR", "/tmp/trueos-spirit-agent-producer-target")
subprocess.run(
    ["cargo", "+nightly-2026-07-10", "test", "--offline", "--manifest-path", str(fixture / "Cargo.toml")],
    cwd=fixture,
    env=env,
    check=True,
)
