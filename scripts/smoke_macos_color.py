#!/usr/bin/env python3
"""End-to-end macOS smoke test for truecolor terminal rendering.

The test starts the real tuimux TUI in a pseudo terminal, has the child shell
emit explicit 24-bit foreground/background SGR sequences, and verifies that the
outer TUI output still contains those SGR sequences. It intentionally sets
NO_COLOR=1 for the parent process because tuimux must preserve colors emitted
by child terminal applications even when the host environment asks ordinary CLI
programs not to colorize their own output.
"""

from __future__ import annotations

import argparse
import fcntl
import os
import pty
import select
import struct
import subprocess
import sys
import termios
import time
from pathlib import Path


FG_MARKER = b"FG_TRUECOLOR"
BG_MARKER = b"BG_TRUECOLOR"
DEFAULT_MARKER = b"DEFAULT_COLOR"
FG_SGR = b"\x1b[38;2;12;34;56m"
BG_SGR = b"\x1b[48;2;78;90;123m"
ROWS = 24
COLS = 100


def build_parser() -> argparse.ArgumentParser:
    repo_root = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(
        description="Smoke-test tuimux truecolor preservation in the real TUI."
    )
    parser.add_argument(
        "--binary",
        type=Path,
        default=repo_root / "target" / "debug" / "tuimux",
        help="tuimux binary to run. Default: target/debug/tuimux",
    )
    parser.add_argument(
        "--session",
        default=f"color-smoke-{os.getpid()}",
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
        env["NO_COLOR"] = "1"
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

    def wait_contains(self, marker: bytes, timeout: float) -> bool:
        deadline = time.time() + timeout
        while time.time() < deadline:
            self.read_for(0.1)
            if marker in self.buffer:
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
        raise SystemExit("this smoke test requires macOS PTY behavior")


def stop_daemon(binary: Path, session: str) -> None:
    subprocess.run(
        [str(binary), "--stop-server", "--session", session],
        cwd=Path(__file__).resolve().parents[1],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False,
    )


def marker_prefix(output: bytes, marker: bytes, width: int = 120) -> bytes:
    idx = output.rfind(marker)
    if idx == -1:
        return b""
    return output[max(0, idx - width) : idx]


def main() -> int:
    parser = build_parser()
    args = parser.parse_args()
    binary = args.binary.resolve()
    if not binary.exists():
        parser.error(f"tuimux binary does not exist: {binary}")
    require_macos()

    client = PtyClient(binary, args.session)
    try:
        client.read_for(1.5)
        command = (
            "printf '\\033[2J\\033[H"
            "\\033[38;2;12;34;56mFG_TRUECOLOR\\033[0m\\r\\n"
            "\\033[48;2;78;90;123mBG_TRUECOLOR\\033[0m\\r\\n"
            "DEFAULT_COLOR\\r\\n'\r"
        )
        client.write(command.encode())
        if not client.wait_contains(DEFAULT_MARKER, args.timeout):
            print("default-color marker did not appear in tuimux output", file=sys.stderr)
            print(client.tail(), file=sys.stderr)
            return 1
        client.read_for(0.6)

        output = bytes(client.buffer)
        fg_prefix = marker_prefix(output, FG_MARKER)
        bg_prefix = marker_prefix(output, BG_MARKER)
        fg_idx = output.rfind(FG_MARKER)
        bg_idx = output.rfind(BG_MARKER)
        default_idx = output.rfind(DEFAULT_MARKER)

        if fg_idx == -1 or FG_SGR not in fg_prefix:
            print("truecolor foreground SGR was not preserved before FG marker", file=sys.stderr)
            print(client.tail(), file=sys.stderr)
            return 1
        if bg_idx == -1 or BG_SGR not in bg_prefix:
            print("truecolor background SGR was not preserved before BG marker", file=sys.stderr)
            print(client.tail(), file=sys.stderr)
            return 1
        if default_idx == -1 or not (fg_idx < bg_idx < default_idx):
            print("color markers appeared out of order", file=sys.stderr)
            print(client.tail(), file=sys.stderr)
            return 1

        after_bg = output[bg_idx + len(BG_MARKER) : default_idx]
        if b"\x1b[49m" not in after_bg and b"\x1b[0m" not in after_bg:
            print("background color was not reset before default-color text", file=sys.stderr)
            print(client.tail(), file=sys.stderr)
            return 1
        if BG_SGR in after_bg or FG_SGR in after_bg:
            print("truecolor SGR leaked into the default-color line", file=sys.stderr)
            print(client.tail(), file=sys.stderr)
            return 1

        print("OK macOS truecolor smoke")
        print("foreground truecolor SGR: observed")
        print("background truecolor SGR: observed")
        print("default color reset: observed")
        print("NO_COLOR parent override: observed")
        return 0
    finally:
        client.close()
        if not args.keep_daemon:
            stop_daemon(binary, args.session)


if __name__ == "__main__":
    raise SystemExit(main())
