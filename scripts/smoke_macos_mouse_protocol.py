#!/usr/bin/env python3
"""End-to-end macOS smoke test for child mouse protocol handling.

The test starts the real tuimux TUI in a pseudo terminal and runs a raw child
program that enables SGR mouse tracking. A normal mouse click must be forwarded
to that child. A Shift-drag over visible text must instead stay in tuimux as a
selection override, and Ctrl-C must copy the selected text to the macOS system
clipboard without leaking Ctrl-C to the child process.
"""

from __future__ import annotations

import argparse
import fcntl
import os
import pty
import re
import select
import shlex
import shutil
import struct
import subprocess
import sys
import termios
import time
from pathlib import Path


TARGET = "TUIMUX_SHIFT_MOUSE_SELECT_TARGET"
READY = "MOUSE_PROTOCOL_READY"
FORWARD_OK = "MOUSE_FORWARD_OK"
SHIFT_LEAK = "MOUSE_SHIFT_LEAK"
CTRL_C_LEAK = "MOUSE_CTRL_C_LEAK"
SENTINEL = "TUIMUX_MOUSE_PROTOCOL_SENTINEL"
ROWS = 24
COLS = 100


def build_parser() -> argparse.ArgumentParser:
    repo_root = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(
        description="Smoke-test tuimux mouse routing when a child enables mouse tracking."
    )
    parser.add_argument(
        "--binary",
        type=Path,
        default=repo_root / "target" / "debug" / "tuimux",
        help="tuimux binary to run. Default: target/debug/tuimux",
    )
    parser.add_argument(
        "--session",
        default=f"mouse-protocol-smoke-{os.getpid()}",
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
        raise SystemExit("this smoke test requires macOS because it verifies pbcopy/pbpaste")
    for program in ("pbcopy", "pbpaste"):
        if shutil.which(program) is None:
            raise SystemExit(f"required command not found: {program}")


def set_clipboard(text: str) -> None:
    subprocess.run(["pbcopy"], input=text, text=True, check=True)


def get_clipboard() -> str:
    return subprocess.check_output(["pbpaste"], text=True)


def stop_daemon(binary: Path, session: str) -> None:
    subprocess.run(
        [str(binary), "--stop-server", "--session", session],
        cwd=Path(__file__).resolve().parents[1],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False,
    )


def mouse_probe_source() -> str:
    return """
import os
import re
import select
import sys
import termios
import time
import tty

fd = sys.stdin.fileno()
old = termios.tcgetattr(fd)
mouse_re = re.compile(rb"\\x1b\\[<([0-9]+);([0-9]+);([0-9]+)([Mm])")
target = "TUIMUX_" + "SHIFT_MOUSE_" + "SELECT_TARGET"
ready_marker = "MOUSE_" + "PROTOCOL_" + "READY"
forward_ok = "MOUSE_" + "FORWARD_" + "OK"
shift_leak = "MOUSE_" + "SHIFT_" + "LEAK"
ctrl_c_leak = "MOUSE_" + "CTRL_C_" + "LEAK"

try:
    tty.setraw(fd)
    os.write(sys.stdout.fileno(), b"\\x1b[2J\\x1b[H" + target.encode() + b"\\r\\n")
    os.write(sys.stdout.fileno(), b"\\x1b[?1002h\\x1b[?1006h" + ready_marker.encode() + b"\\r\\n")
    forwarded = False
    data = b""
    deadline = time.time() + 12.0
    while time.time() < deadline:
        ready, _, _ = select.select([fd], [], [], 0.1)
        if not ready:
            continue
        chunk = os.read(fd, 1024)
        if not chunk:
            break
        if b"q" in chunk:
            break
        if b"\\x03" in chunk:
            os.write(sys.stdout.fileno(), ctrl_c_leak.encode() + b"\\r\\n")
            continue
        data += chunk
        while True:
            match = mouse_re.search(data)
            if not match:
                data = data[-32:]
                break
            code = int(match.group(1))
            encoded = match.group(0).decode("ascii", "replace")
            data = data[match.end():]
            if not forwarded:
                os.write(sys.stdout.fileno(), forward_ok.encode() + b":" + encoded.encode() + b"\\r\\n")
                forwarded = True
            else:
                os.write(sys.stdout.fileno(), shift_leak.encode() + b":" + encoded.encode() + b"\\r\\n")
            if code & 4:
                os.write(sys.stdout.fileno(), shift_leak.encode() + b":" + encoded.encode() + b"\\r\\n")
finally:
    os.write(sys.stdout.fileno(), b"\\x1b[?1006l\\x1b[?1002l")
    termios.tcsetattr(fd, termios.TCSADRAIN, old)
"""


def mouse_probe_command(probe_file: Path) -> str:
    return f"python3 {shlex.quote(str(probe_file))}"


def main() -> int:
    parser = build_parser()
    args = parser.parse_args()
    binary = args.binary.resolve()
    if not binary.exists():
        parser.error(f"tuimux binary does not exist: {binary}")
    require_macos()
    set_clipboard(SENTINEL)
    probe_file = Path("/tmp") / f"tuimux-mouse-probe-{os.getpid()}.py"
    probe_file.write_text(mouse_probe_source())

    client = PtyClient(binary, args.session)
    try:
        client.read_for(1.5)
        client.write((mouse_probe_command(probe_file) + "\r").encode())
        if not client.wait_contains(READY, args.timeout):
            print("child mouse probe did not become ready", file=sys.stderr)
            print(client.tail(), file=sys.stderr)
            return 1

        # Normal click: with child mouse tracking active, this must be forwarded
        # to the child process rather than starting tuimux text selection.
        client.write(b"\x1b[<0;10;4M")
        if not client.wait_contains(FORWARD_OK, args.timeout):
            print("normal mouse event did not reach mouse-tracking child", file=sys.stderr)
            print(client.tail(), file=sys.stderr)
            return 1

        # Shift-drag: crossterm reports modifier bit 4. tuimux should treat this
        # as a selection override even while the child owns normal mouse events.
        y = 1
        x1 = 1
        x2 = len(TARGET)
        shifted_drag = (
            f"\x1b[<4;{x1};{y}M"
            f"\x1b[<36;{x2};{y}M"
            f"\x1b[<4;{x2};{y}m"
        )
        before_shift = len(client.buffer)
        client.write(shifted_drag.encode())
        client.read_for(0.7)
        shift_frame = bytes(client.buffer[before_shift:])
        if SHIFT_LEAK.encode() in shift_frame or SHIFT_LEAK.encode() in client.buffer:
            print("Shift-drag leaked to mouse-tracking child", file=sys.stderr)
            print(client.tail(), file=sys.stderr)
            return 1

        client.write(b"\x03")
        time.sleep(0.5)
        client.read_for(0.2)
        if CTRL_C_LEAK.encode() in client.buffer:
            print("Ctrl-C leaked to mouse-tracking child while selection was active", file=sys.stderr)
            print(client.tail(), file=sys.stderr)
            return 1

        copied = get_clipboard()
        if copied != TARGET:
            print(f"clipboard mismatch: expected {TARGET!r}, got {copied!r}", file=sys.stderr)
            print(client.tail(), file=sys.stderr)
            return 1

        # Clear selection and let the child restore terminal mouse mode before
        # the TUI client exits.
        client.write(b"q")
        client.read_for(0.5)

        print("OK macOS mouse protocol smoke")
        print("normal mouse forwarding: observed")
        print("Shift-drag selection override: copied through system clipboard")
        print("Ctrl-C with selection: not leaked to child")
        return 0
    finally:
        client.close()
        probe_file.unlink(missing_ok=True)
        if not args.keep_daemon:
            stop_daemon(binary, args.session)


if __name__ == "__main__":
    raise SystemExit(main())
