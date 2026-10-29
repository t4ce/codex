#!/usr/bin/env python3
"""Exercise the actual native prompt inbox without the host Hyper overlay.

Only the filesystem reader is replaced; production scheduling, scope retention,
framing, acceptance, and cancellation behavior are imported unchanged.
The full ChatWidget integration cases live in composer_submission.rs.
"""

import os
from pathlib import Path
import subprocess
import tempfile


TUI = Path(__file__).resolve().parents[2]
fixture = Path(tempfile.mkdtemp(prefix="trueos-prompt-ingress-"))
(fixture / "src").mkdir()
(fixture / "src/lib.rs").write_text(
    '#![allow(dead_code, unexpected_cfgs)]\n'
    f'#[path = "{TUI / "src/app/trueos_prompt.rs"}"]\n'
    'mod trueos_prompt;\n'
)
(fixture / "Cargo.toml").write_text('''[package]
name = "trueos-prompt-ingress-fixture"
version = "0.0.0"
edition = "2024"

[dependencies]
tokio = { version = "=1.54.1", features = ["rt", "macros", "sync", "time", "test-util"] }
tracing = "=0.1.44"
''')
env = dict(os.environ)
env.setdefault("CARGO_TARGET_DIR", "/tmp/trueos-prompt-ingress-target")
print(f"Native prompt ingress fixture: {fixture}", flush=True)
subprocess.run(
    ["cargo", "+nightly-2026-07-10", "test", "--offline", "--manifest-path", str(fixture / "Cargo.toml")],
    cwd=fixture,
    env=env,
    check=True,
)
