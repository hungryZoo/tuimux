#!/usr/bin/env python3
"""End-to-end macOS smoke test for host resize propagation.

The test starts the real tuimux TUI in a pseudo terminal, runs a child program
that reports its terminal size, then resizes the host PTY. The child must receive
SIGWINCH through tuimux's PTY resize path and report the new size.
"""

from __future__ import annotations

import argparse
import fcntl
import os
import pty
import select
import signal
import struct
import subprocess
import sys
import termios
import time
from pathlib import Path


INITIAL_SIZE = "INITIAL:22x80"
RESIZED_SIZE = "RESIZED:30x92"
ROWS = 24
COLS = 80
RESIZED_ROWS = 32
RESIZED_COLS = 120


def build_parser() -> argparse.ArgumentParser:
    repo_root = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(
        description="Smoke-test tuimux host resize propagation to child PTYs."
    )
    parser.add_argument(
        "--binary",
        type=Path,
        default=repo_root / "target" / "debug" / "tuimux",
        help="tuimux binary to run. Default: target/debug/tuimux",
    )
    parser.add_argument(
        "--session",
        default=f"resize-smoke-{os.getpid()}",
        help="temporary tuimux session name",
    )
    parser.add_argument(
        "--timeout",
        type=float,
        default=8.0,
        help="seconds to wait for checkpoints",
    )
    parser.add_argument(
        "--keep-daemon",
        action="store_true",
        help="leave the temporary daemon running for debugging",
    )
    return parser


class PtyClient:
    def __init__(self, binary: Path, session: str) -> None:
        self.binary = binary
        self.session = session
        self.repo_root = Path(__file__).resolve().parents[1]
        self.master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
        env = os.environ.copy()
        env["TERM"] = "xterm-256color"
        env["COLORTERM"] = "truecolor"
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

    def resize(self, rows: int, cols: int) -> None:
        fcntl.ioctl(self.master, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
        if self.process.poll() is None:
            os.kill(self.process.pid, signal.SIGWINCH)

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
        return bytes(self.buffer[-5000:]).decode("utf-8", "replace")


def require_macos() -> None:
    if sys.platform != "darwin":
        raise SystemExit("this smoke test requires macOS PTY resize behavior")


def stop_daemon(binary: Path, session: str) -> None:
    subprocess.run(
        [str(binary), "--stop-server", "--session", session],
        cwd=Path(__file__).resolve().parents[1],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False,
    )


def resize_probe_source() -> str:
    return """
import shutil
import signal
import time

pending_resize = False

def emit(label):
    size = shutil.get_terminal_size((0, 0))
    print(f"{label}:{size.lines}x{size.columns}", flush=True)

def on_winch(signum, frame):
    global pending_resize
    pending_resize = True

signal.signal(signal.SIGWINCH, on_winch)
emit("INITIAL")
deadline = time.time() + 8.0
while time.time() < deadline:
    if pending_resize:
        pending_resize = False
        emit("RESIZED")
    time.sleep(0.05)
"""


def main() -> int:
    parser = build_parser()
    args = parser.parse_args()
    binary = args.binary.resolve()
    if not binary.exists():
        parser.error(f"tuimux binary does not exist: {binary}")
    require_macos()

    probe_file = Path("/tmp") / f"tuimux-resize-probe-{os.getpid()}.py"
    probe_file.write_text(resize_probe_source())
    client = PtyClient(binary, args.session)
    try:
        client.read_for(1.5)
        client.write(f"python3 {probe_file}\r".encode())
        if not client.wait_contains(INITIAL_SIZE, args.timeout):
            print("child did not report the initial PTY size", file=sys.stderr)
            print(client.tail(), file=sys.stderr)
            return 1

        client.resize(RESIZED_ROWS, RESIZED_COLS)
        if not client.wait_contains(RESIZED_SIZE, args.timeout):
            print("child did not observe the resized PTY size", file=sys.stderr)
            print(client.tail(), file=sys.stderr)
            return 1

        print("OK macOS resize smoke")
        print(f"initial child PTY size: {INITIAL_SIZE}")
        print(f"resized child PTY size: {RESIZED_SIZE}")
        print("child SIGWINCH after host resize: observed")
        return 0
    finally:
        client.close()
        probe_file.unlink(missing_ok=True)
        if not args.keep_daemon:
            stop_daemon(binary, args.session)


if __name__ == "__main__":
    raise SystemExit(main())
