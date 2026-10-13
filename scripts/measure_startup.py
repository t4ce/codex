#!/usr/bin/env python3
"""Measure an instrumented Codex in a PTY; never submit a model prompt.

Build: cd codex-rs && cargo run --bin codex --no-default-features -- --no-daemon
The probe runs the resulting binary directly, excluding Cargo/build time.
"""
import argparse
import errno
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import shutil
import signal
import struct
import subprocess
import tempfile
import termios
import time

QUERIES = {
    b'\x1b[6n': b'\x1b[1;1R',
    b'\x1b]10;?\x1b\\': b'\x1b]10;rgb:ffff/ffff/ffff\x1b\\',
    b'\x1b]11;?\x1b\\': b'\x1b]11;rgb:0000/0000/0000\x1b\\',
    b'\x1b[?u': b'\x1b[?0u',
    b'\x1b[c': b'\x1b[?1;2c',
    b'\x1b[>c': b'\x1b[>0;100;0c',
}


def rows(path):
    if not path.exists():
        return []
    result = []
    for line in path.read_text().splitlines():
        parts = line.split('\t')
        if len(parts) == 4:
            result.append(dict(pid=int(parts[0]), elapsed_ms=float(parts[1]),
                               delta_ms=float(parts[2]), stage=parts[3]))
    return result


def run(binary, home, cwd, output, name, replies, timeout):
    trace = output / (name + '.tsv')
    trace.write_text('')
    transcript = output / (name + '.terminal')
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 40, 120, 0, 0))
    env = os.environ.copy()
    env.update(CODEX_HOME=str(home), CODEX_STARTUP_TIMING_FILE=str(trace), TERM='xterm-256color')
    for key in ['OPENAI_API_KEY', 'CODEX_API_KEY', 'CODEX_EXEC_SERVER_URL', 'SSH_TTY', 'SSH_CONNECTION']:
        env.pop(key, None)

    def controlling_terminal():
        os.setsid()
        fcntl.ioctl(0, termios.TIOCSCTTY, 0)

    start = time.perf_counter()
    proc = subprocess.Popen([str(binary), '--no-daemon'], cwd=cwd, env=env,
                            stdin=slave, stdout=slave, stderr=slave,
                            preexec_fn=controlling_terminal)
    os.close(slave)
    pending = b''
    terminal_bytes = bytearray()
    milestone = None
    try:
        while time.perf_counter() - start < timeout and proc.poll() is None:
            readable, _, _ = select.select([master], [], [], 0.02)
            if readable:
                try:
                    data = os.read(master, 65536)
                except OSError as error:
                    if error.errno == errno.EIO:
                        break
                    raise
                terminal_bytes.extend(data)
                pending += data
                if replies:
                    for query, reply in QUERIES.items():
                        while query in pending:
                            os.write(master, reply)
                            pending = pending.replace(query, b'', 1)
                pending = pending[-256:]
            stages = {row['stage'] for row in rows(trace) if row['pid'] == proc.pid}
            if 'session_prompt_ready' in stages:
                milestone = 'session_prompt_ready'
                break
            if 'onboarding_visible' in stages:
                milestone = 'onboarding_visible'
                break
        elapsed = (time.perf_counter() - start) * 1000
        # Verify local editing without Enter or any request to a model.
        edit_visible = False
        if milestone == 'session_prompt_ready':
            os.write(master, b'startup-timing-probe')
            until = time.perf_counter() + 1
            while time.perf_counter() < until:
                readable, _, _ = select.select([master], [], [], 0.05)
                if readable:
                    try:
                        terminal_bytes.extend(os.read(master, 65536))
                    except OSError as error:
                        if error.errno == errno.EIO:
                            break
                        raise
                if b'startup-timing-probe' in terminal_bytes:
                    edit_visible = True
                    break
        return dict(name=name, terminal_replies=replies, milestone=milestone,
                    launch_wall_ms=round(elapsed, 3), typed_text_visible=edit_visible,
                    stages=[row for row in rows(trace) if row['pid'] == proc.pid])
    finally:
        transcript.write_bytes(terminal_bytes)
        if proc.poll() is None:
            os.write(master, b'\x03')
            time.sleep(0.2)
            if proc.poll() is None:
                os.write(master, b'\x03')
            try:
                proc.wait(timeout=3)
            except subprocess.TimeoutExpired:
                os.killpg(proc.pid, signal.SIGTERM)
                try:
                    proc.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    os.killpg(proc.pid, signal.SIGKILL)
                    proc.wait()
        # Stop any surviving test helpers in the child's isolated process group.
        try:
            os.killpg(proc.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        os.close(master)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--auth-source', type=Path, help='Copy authentication into the disposable profile; never print it.')
    parser.add_argument('--timeout', type=float, default=60)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    result = []
    with tempfile.TemporaryDirectory(prefix='codex-startup-probe-') as root:
        root = Path(root)
        cwd = root / 'workspace'
        cwd.mkdir()
        home = root / 'fresh-home'
        home.mkdir(mode=0o700)
        (home / 'config.toml').write_text(f'[projects.{json.dumps(str(cwd))}]\ntrust_level = "trusted"\n')
        if args.auth_source:
            shutil.copyfile(args.auth_source, home / 'auth.json')
            (home / 'auth.json').chmod(0o600)
        for name, replies in [('fresh', True), ('repeat', True), ('repeat_no_terminal_replies', False)]:
            print('Measuring', name, flush=True)
            result.append(run(args.binary.resolve(), home, cwd, args.output, name, replies, args.timeout))
            (args.output / 'results.json').write_text(json.dumps(result, indent=2) + '\n')
        unauth = root / 'unauthenticated-home'
        unauth.mkdir(mode=0o700)
        (unauth / 'config.toml').write_text(f'[projects.{json.dumps(str(cwd))}]\ntrust_level = "trusted"\n')
        result.append(run(args.binary.resolve(), unauth, cwd, args.output, 'fresh_unauthenticated', True, args.timeout))
        (args.output / 'results.json').write_text(json.dumps(result, indent=2) + '\n')
    for row in result:
        print(row['name'], row['milestone'], row['launch_wall_ms'], 'ms; editing:', row['typed_text_visible'])


if __name__ == '__main__':
    main()
