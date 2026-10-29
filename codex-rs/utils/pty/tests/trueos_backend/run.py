#!/usr/bin/env python3
"""Run the actual native adapter/process driver without the host workspace overlay.

Only command transport is faked by the adapter's production inline tests. The
SDK contract, process driver, async queues, input policy and reader tasks are
linked unchanged; no kernel, terminal, network or live VM is involved.
"""

import json
import os
from pathlib import Path
import subprocess
import tempfile


HERE = Path(__file__).resolve().parent
CODEX_RS = HERE.parents[3]
REPOSITORIES = CODEX_RS.parent.parent
with tempfile.TemporaryDirectory(prefix="trueos-native-command-adapter-") as temporary:
    fixture = Path(temporary)
    (fixture / "src").mkdir()
    modules = [
        ("command_session", REPOSITORIES / "TRUEOS-Blueprints/api/src/command_session.rs"),
        ("child_fds", CODEX_RS / "utils/pty/src/child_fds.rs"),
        ("process", CODEX_RS / "utils/pty/src/process.rs"),
        ("pty", CODEX_RS / "utils/pty/src/trueos.rs"),
    ]
    source = "#![allow(dead_code, unused_imports)]\nextern crate alloc;\nextern crate self as trueos;\n"
    for module, path in modules:
        source += f"#[path = {json.dumps(str(path))}]\nmod {module};\n"
    # Exercise the unchanged production help renderer and its inline tests,
    # without pulling the unrelated tool/runtime graph into this host fixture.
    help_source = (CODEX_RS / "core/src/tools/handlers/trueos_help.rs").read_text()
    render_start = help_source.index("fn render_help(")
    render_stop = help_source.index('\n#[cfg(target_os = "trueos")]', render_start)
    catalog_start = help_source.index("fn catalog_source()")
    catalog_stop = help_source.index("\nfn text_error(", catalog_start)
    constants = "\n".join(line for line in help_source.splitlines()
                          if line.startswith("const MAX_ALIAS_"))
    source += "mod help_document {\n" + constants + "\n"
    source += 'const SHELL3_GROUPS: &[(&str, &str, &[&str])] = &[];\n'
    source += help_source[catalog_start:catalog_stop] + "\n"
    source += help_source[render_start:render_stop] + "\n"
    source += help_source[help_source.index("#[cfg(test)]\nmod tests {"):] + "\n}\n"
    (fixture / "src/lib.rs").write_text(source)
    (fixture / "Cargo.toml").write_text('''[package]
name = "trueos-native-command-adapter-fixture"
version = "0.0.0"
edition = "2024"

[dependencies]
anyhow = "1"
libc = "0.2"
portable-pty = "=0.9.0"
tokio = { version = "=1.54.1", features = ["rt", "macros", "sync", "time"] }
serde_json = "=1.0.150"

[lints.rust]
unexpected_cfgs = { level = "warn", check-cfg = ['cfg(target_os, values("trueos"))'] }
''')
    env = dict(os.environ)
    env.setdefault("CARGO_TARGET_DIR", "/tmp/trueos-native-command-adapter-target")
    subprocess.run(["cargo", "+nightly-2026-07-10", "test", "--offline",
                    "--manifest-path", str(fixture / "Cargo.toml")],
                   cwd=fixture, env=env, check=True)
