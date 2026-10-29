#!/usr/bin/env python3
"""Exercise the production suggestion handler with host and TRUEOS key rules.

Only the composer/terminal adapters are supplied by the fixture. This avoids
the unrelated host Hyper dependency conflict in the full TUI test graph.
"""

from pathlib import Path
import subprocess
import tempfile


HERE = Path(__file__).resolve().parent
TUI = HERE.parents[1]
source = (TUI / "src/bottom_pane/chat_composer/next_prompt.rs").read_text()
with tempfile.TemporaryDirectory(prefix="codex-next-prompt-keys-") as directory:
    work = Path(directory)
    (work / "next_prompt.rs").write_text(source.split("#[cfg(test)]", 1)[0])
    (work / "harness.rs").write_text((HERE / "harness.rs").read_text())
    for platform, extra in [
        ("host", []),
        ("TRUEOS", ["--cfg", 'target_os="trueos"', "-Aexplicit_builtin_cfgs_in_flags"]),
    ]:
        binary = work / platform
        subprocess.run([
            "rustc", "+nightly-2026-07-10", "--edition=2024", "--test",
            str(work / "harness.rs"), "-o", str(binary), *extra,
        ], check=True)
        print(f"{platform} suggestion keys", flush=True)
        subprocess.run([str(binary)], check=True)
