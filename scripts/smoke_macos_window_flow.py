#!/usr/bin/env python3
"""End-to-end macOS smoke test for tuimux window workflow."""

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


ROWS = 24
COLS = 100
F12 = b"\x1b[24~"


def build_parser() -> argparse.ArgumentParser:
    repo_root = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(
        description="Smoke-test tuimux detach/reattach and window-list workflow."
    )
    parser.add_argument(
        "--binary",
        type=Path,
        default=repo_root / "target" / "debug" / "tuimux",
        help="tuimux binary to run. Default: target/debug/tuimux",
    )
    parser.add_argument(
        "--socket-scope",
        default=f"window-flow-smoke-{os.getpid()}",
        help="temporary tuimux daemon socket scope",
    )
    parser.add_argument(
        "--session",
        dest="socket_scope",
        help=argparse.SUPPRESS,
    )
    parser.add_argument(
        "--timeout",
        type=float,
        default=8.0,
        help="seconds to wait for each checkpoint",
    )
    parser.add_argument(
        "--keep-daemon",
        action="store_true",
        help="leave the temporary daemon running for debugging",
    )
    return parser


class PtyClient:
    def __init__(self, binary: Path, socket_scope: str) -> None:
        self.binary = binary
        self.socket_scope = socket_scope
        self.repo_root = Path(__file__).resolve().parents[1]
        self.master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
        env = os.environ.copy()
        env["TERM"] = "xterm-256color"
        env["COLORTERM"] = "truecolor"
        self.process = subprocess.Popen(
            [str(binary), "--socket-scope", socket_scope],
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

    def clear_buffer(self) -> None:
        self.buffer.clear()

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


def stop_daemon(binary: Path, socket_scope: str) -> None:
    subprocess.run(
        [str(binary), "--stop-server", "--socket-scope", socket_scope],
        cwd=Path(__file__).resolve().parents[1],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False,
    )


def wait_or_fail(client: PtyClient, text: str, timeout: float, label: str) -> None:
    if not client.wait_contains(text, timeout):
        raise RuntimeError(f"{label} did not appear; tail:\n{client.tail()}")


def shell(client: PtyClient, command: str) -> None:
    client.write((command + "\r").encode())


def paste_shell(client: PtyClient, command: str) -> None:
    client.write(f"\x1b[200~{command}\x1b[201~".encode())
    client.read_for(0.2)
    client.write(b"\r")


def enter_navigation(client: PtyClient, timeout: float) -> None:
    client.clear_buffer()
    client.write(F12)
    wait_or_fail(client, "WINDOWS", timeout, "window list")
    wait_or_fail(client, "+ new", timeout, "new-window row")


def run_window_flow(client: PtyClient, timeout: float) -> None:
    enter_navigation(client, timeout)

    client.clear_buffer()
    client.write(b"n")
    wait_or_fail(client, "2:", timeout, "second window row")

    client.clear_buffer()
    client.write(b"x")
    wait_or_fail(client, "1:", timeout, "remaining first window row")


def detach(client: PtyClient, timeout: float) -> None:
    client.clear_buffer()
    client.write(b"d")
    wait_or_fail(
        client,
        "tuimux: closed native multiplexer UI.",
        timeout,
        "detach message",
    )
    deadline = time.time() + timeout
    while time.time() < deadline:
        if client.process.poll() is not None:
            return
        time.sleep(0.1)
    raise RuntimeError("client process did not exit after detach")


def main() -> int:
    parser = build_parser()
    args = parser.parse_args()
    binary = args.binary.resolve()
    if not binary.exists():
        parser.error(f"tuimux binary does not exist: {binary}")
    if sys.platform != "darwin":
        raise SystemExit("this smoke test currently targets macOS")

    try:
        first = PtyClient(binary, args.socket_scope)
        try:
            first.read_for(1.5)
            shell(
                first,
                "printf '\\033[2J\\033[H'; "
                "export TUIMUX_PERSIST_MARK=alive; "
                "printf 'TUIMUX_PERSIST_SET\\n'",
            )
            wait_or_fail(first, "TUIMUX_PERSIST_SET", args.timeout, "persist setup")
            run_window_flow(first, args.timeout)
            detach(first, args.timeout)
        finally:
            first.close()

        second = PtyClient(binary, args.socket_scope)
        try:
            second.read_for(1.5)
            wait_or_fail(second, "TUIMUX_PERSIST_SET", args.timeout, "reattached screen")
            second.clear_buffer()
            paste_shell(
                second,
                "printf '\\033[2J\\033[H'; "
                "printf 'PERSIST:%s\\n' \"$TUIMUX_PERSIST_MARK\"",
            )
            wait_or_fail(
                second,
                "PERSIST:alive",
                args.timeout,
                "reattached shell state",
            )
            enter_navigation(second, args.timeout)
            wait_or_fail(second, "1:", args.timeout, "reattached window list")
            detach(second, args.timeout)
        finally:
            second.close()

        print("OK macOS window flow smoke")
        print("window workflow: new, kill")
        print("detach/reattach: shell state persisted")
        return 0
    except RuntimeError as exc:
        print(exc, file=sys.stderr)
        return 1
    finally:
        if not args.keep_daemon:
            stop_daemon(binary, args.socket_scope)


if __name__ == "__main__":
    raise SystemExit(main())
