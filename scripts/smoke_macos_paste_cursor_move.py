#!/usr/bin/env python3
"""Verify paste highlight clears by moving the shell cursor to the click cell."""

from __future__ import annotations

import argparse
import fcntl
import os
import pty
import re
import select
import struct
import subprocess
import sys
import termios
import time
from pathlib import Path


ROWS = 24
COLS = 100
PAYLOAD = "TUIMUX_CLICK_CURSOR_MOVE_FINAL"
CLICK_COL = 36
CLICK_ROW = 1


def build_parser() -> argparse.ArgumentParser:
    repo_root = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(
        description="Smoke-test paste highlight clearing through cursor-moving left click."
    )
    parser.add_argument(
        "--binary",
        type=Path,
        default=repo_root / "target" / "debug" / "tuimux",
    )
    parser.add_argument(
        "--session",
        default=f"paste-cursor-move-smoke-{os.getpid()}",
    )
    parser.add_argument("--timeout", type=float, default=1.5)
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
        env["SHELL"] = "/bin/zsh"
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

    def read_for(self, seconds: float) -> bytes:
        start = len(self.buffer)
        deadline = time.time() + seconds
        while time.time() < deadline:
            timeout = min(0.05, max(0.0, deadline - time.time()))
            ready, _, _ = select.select([self.master], [], [], timeout)
            if not ready:
                continue
            try:
                chunk = os.read(self.master, 65536)
            except OSError:
                break
            if not chunk:
                break
            self.buffer.extend(chunk)
        return bytes(self.buffer[start:])

    def write(self, data: bytes) -> None:
        os.write(self.master, data)

    def close(self) -> None:
        self.process.terminate()
        try:
            self.process.wait(timeout=1)
        except subprocess.TimeoutExpired:
            self.process.kill()
        try:
            os.close(self.master)
        except OSError:
            pass


def stop_daemon(binary: Path, session: str) -> None:
    subprocess.run(
        [str(binary), "--session", session, "--stop-server"],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False,
    )


def main() -> int:
    if sys.platform != "darwin":
        print("SKIP: macOS-only smoke", file=sys.stderr)
        return 0

    args = build_parser().parse_args()
    if not args.binary.exists():
        print(f"missing binary: {args.binary}", file=sys.stderr)
        return 1

    client = PtyClient(args.binary, args.session)
    try:
        client.read_for(args.timeout)
        client.write(b"\x15")
        client.read_for(0.2)

        client.write(f"\x1b[200~{PAYLOAD}\x1b[201~".encode())
        after_paste = client.read_for(0.8)

        click = f"\x1b[<0;{CLICK_COL};{CLICK_ROW}M\x1b[<0;{CLICK_COL};{CLICK_ROW}m"
        client.write(click.encode())
        after_click = client.read_for(0.8)

        highlighted = bool(re.search(rb"\x1b\[[0-9;]*7m" + PAYLOAD.encode(), after_paste))
        still_highlighted = bool(
            re.search(rb"\x1b\[[0-9;]*7m" + PAYLOAD.encode(), after_click)
        )
        unhighlighted = PAYLOAD.encode() in after_click and not still_highlighted
        mouse_leaked = b"\x1b[<" in after_click or b"!!#!!" in after_click
        cursor_positions = re.findall(rb"\x1b\[(\d+);(\d+)H", after_click)
        final_cursor = cursor_positions[-1] if cursor_positions else None
        clicked_column = final_cursor == (str(CLICK_ROW).encode(), str(CLICK_COL).encode())

        if highlighted and unhighlighted and not mouse_leaked and clicked_column:
            print("OK macOS paste cursor-move smoke")
            print("paste highlight: reverse video observed before click")
            print(f"left click moved cursor to column {CLICK_COL}")
            print("paste highlight cleared without mouse escape leak")
            return 0

        print("paste cursor-move smoke failed", file=sys.stderr)
        print(f"highlighted={highlighted}", file=sys.stderr)
        print(f"unhighlighted={unhighlighted}", file=sys.stderr)
        print(f"still_highlighted={still_highlighted}", file=sys.stderr)
        print(f"mouse_leaked={mouse_leaked}", file=sys.stderr)
        print(f"final_cursor={final_cursor}", file=sys.stderr)
        return 1
    finally:
        client.close()
        stop_daemon(args.binary, args.session)


if __name__ == "__main__":
    raise SystemExit(main())
