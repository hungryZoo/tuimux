#!/usr/bin/env python3
"""End-to-end macOS smoke test for child OSC 52 clipboard copy."""

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
CLIP_TEXT = "OSC52_COPY_OK"
CLIP_B64 = "T1NDNTJfQ09QWV9PSw=="


def build_parser() -> argparse.ArgumentParser:
    repo_root = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(
        description="Smoke-test child OSC 52 copy into the macOS clipboard."
    )
    parser.add_argument(
        "--binary",
        type=Path,
        default=repo_root / "target" / "debug" / "tuimux",
        help="tuimux binary to run. Default: target/debug/tuimux",
    )
    parser.add_argument(
        "--session",
        default=f"osc52-clipboard-smoke-{os.getpid()}",
        help="temporary tuimux session name",
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


def stop_daemon(binary: Path, session: str) -> None:
    subprocess.run(
        [str(binary), "--stop-server", "--session", session],
        cwd=Path(__file__).resolve().parents[1],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False,
    )


def set_clipboard(text: str) -> None:
    subprocess.run(["pbcopy"], input=text.encode(), check=True)


def read_clipboard() -> str:
    return subprocess.check_output(["pbpaste"]).decode("utf-8", "replace")


def paste_shell(client: PtyClient, command: str) -> None:
    client.write(f"\x1b[200~{command}\x1b[201~".encode())
    client.read_for(0.2)
    client.write(b"\r")


def wait_clipboard(expected: str, timeout: float) -> bool:
    deadline = time.time() + timeout
    while time.time() < deadline:
        if read_clipboard() == expected:
            return True
        time.sleep(0.1)
    return False


def main() -> int:
    parser = build_parser()
    args = parser.parse_args()
    binary = args.binary.resolve()
    if not binary.exists():
        parser.error(f"tuimux binary does not exist: {binary}")
    if sys.platform != "darwin":
        raise SystemExit("this smoke test currently targets macOS")

    client = PtyClient(binary, args.session)
    try:
        set_clipboard("TUIMUX_OSC52_BEFORE")
        client.read_for(1.5)
        paste_shell(
            client,
            "python3 -c 'import sys,time; "
            f"sys.stdout.write(\"\\033]52;c;{CLIP_B64}\\007\"); "
            "sys.stdout.flush(); time.sleep(0.2)'",
        )

        if not wait_clipboard(CLIP_TEXT, args.timeout):
            raise RuntimeError(
                "OSC 52 clipboard copy did not reach pbpaste; "
                f"clipboard={read_clipboard()!r}; tail:\n{client.tail()}"
            )

        print("OK macOS OSC 52 clipboard smoke")
        print(f"clipboard: {CLIP_TEXT}")
        return 0
    except RuntimeError as exc:
        print(exc, file=sys.stderr)
        return 1
    finally:
        client.close()
        if not args.keep_daemon:
            stop_daemon(binary, args.session)


if __name__ == "__main__":
    raise SystemExit(main())
