#!/usr/bin/env python3
"""End-to-end macOS smoke test for the no-tmux default runtime path."""

from __future__ import annotations

import argparse
import fcntl
import os
import pty
import select
import shutil
import struct
import subprocess
import sys
import termios
import time
from pathlib import Path


ROWS = 24
COLS = 100
NO_TMUX_PATH = "/usr/bin:/bin:/usr/sbin:/sbin"


def build_parser() -> argparse.ArgumentParser:
    repo_root = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(
        description="Smoke-test tuimux default TUI with tmux absent from PATH."
    )
    parser.add_argument(
        "--binary",
        type=Path,
        default=repo_root / "target" / "debug" / "tuimux",
        help="tuimux binary to run. Default: target/debug/tuimux",
    )
    parser.add_argument(
        "--session",
        default=f"no-tmux-smoke-{os.getpid()}",
        help="temporary tuimux session name",
    )
    parser.add_argument(
        "--timeout",
        type=float,
        default=8.0,
        help="seconds to wait for the TUI checkpoint",
    )
    parser.add_argument(
        "--keep-daemon",
        action="store_true",
        help="leave the temporary daemon running for debugging",
    )
    return parser


def no_tmux_env() -> dict[str, str]:
    env = {
        "PATH": NO_TMUX_PATH,
        "SHELL": "/bin/sh",
        "TERM": "xterm-256color",
        "COLORTERM": "truecolor",
    }
    for key in ("HOME", "USER", "TMPDIR"):
        if value := os.environ.get(key):
            env[key] = value
    return env


class PtyClient:
    def __init__(self, binary: Path, session: str, env: dict[str, str]) -> None:
        self.binary = binary
        self.session = session
        self.repo_root = Path(__file__).resolve().parents[1]
        self.master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
        self.process = subprocess.Popen(
            [str(binary), "--session", session],
            cwd=self.repo_root,
            stdin=slave,
            stdout=slave,
            stderr=slave,
            env=env,
            close_fds=True,
        )
        os.close(slave)
        os.set_blocking(self.master, False)
        self.buffer = bytearray()

    def read_for(self, seconds: float) -> None:
        deadline = time.time() + seconds
        while time.time() < deadline:
            timeout = min(0.05, max(0.0, deadline - time.time()))
            readable, _, _ = select.select([self.master], [], [], timeout)
            if not readable:
                continue
            try:
                chunk = os.read(self.master, 8192)
            except (BlockingIOError, OSError):
                break
            if not chunk:
                break
            self.buffer.extend(chunk)

    def wait_contains(self, text: str, timeout: float) -> bool:
        deadline = time.time() + timeout
        needle = text.encode()
        while time.time() < deadline:
            self.read_for(0.1)
            if needle in self.buffer:
                return True
        return False

    def write(self, data: bytes) -> None:
        os.write(self.master, data)

    def close(self) -> None:
        if self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=2)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=2)
        try:
            os.close(self.master)
        except OSError:
            pass

    def tail(self) -> str:
        return bytes(self.buffer[-4000:]).decode("utf-8", "replace")


def run_capture(binary: Path, env: dict[str, str], *args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [str(binary), *args],
        cwd=Path(__file__).resolve().parents[1],
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )


def stop_daemon(binary: Path, session: str, env: dict[str, str]) -> None:
    subprocess.run(
        [str(binary), "--stop-server", "--session", session],
        cwd=Path(__file__).resolve().parents[1],
        env=env,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False,
    )


def main() -> int:
    parser = build_parser()
    args = parser.parse_args()
    binary = args.binary.resolve()
    if not binary.exists():
        parser.error(f"tuimux binary does not exist: {binary}")
    if sys.platform != "darwin":
        raise SystemExit("this smoke test currently targets macOS")

    env = no_tmux_env()
    if shutil.which("tmux", path=env["PATH"]) is not None:
        raise SystemExit(f"tmux unexpectedly exists in smoke PATH={env['PATH']}")

    doctor = run_capture(binary, env, "--doctor")
    doctor_output = doctor.stdout + doctor.stderr
    if doctor.returncode != 0 or "native tuimux does not require tmux" not in doctor_output:
        print("doctor did not pass without tmux", file=sys.stderr)
        print(doctor_output, file=sys.stderr)
        return 1

    native_client = run_capture(binary, env, "--native-client", "--session", args.session)
    native_output = native_client.stdout + native_client.stderr
    if native_client.returncode == 0 or "--native-client` requires tmux" not in native_output:
        print("native-client fallback did not fail clearly without tmux", file=sys.stderr)
        print(native_output, file=sys.stderr)
        return 1

    client = PtyClient(binary, args.session, env)
    try:
        client.read_for(1.5)
        marker = "NO_TMUX_NATIVE_TUI_OK"
        client.write(f"printf '\\033[2J\\033[H{marker}\\n'\r".encode())
        if not client.wait_contains(marker, args.timeout):
            print("default TUI did not run a PTY shell without tmux", file=sys.stderr)
            print(client.tail(), file=sys.stderr)
            return 1

        print("OK macOS no-tmux smoke")
        print("doctor: native runtime OK without tmux")
        print("native-client fallback: fails clearly without tmux")
        print("default TUI: PTY shell ran without tmux")
        return 0
    finally:
        client.close()
        if not args.keep_daemon:
            stop_daemon(binary, args.session, env)


if __name__ == "__main__":
    raise SystemExit(main())
