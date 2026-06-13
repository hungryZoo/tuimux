#!/usr/bin/env python3
"""End-to-end macOS smoke test for tuimux scrollback navigation.

The test starts the real tuimux TUI in a pseudo terminal, creates enough shell
output to fill vt100 scrollback, then verifies that mouse wheel, PageUp, Home,
and End navigate the active terminal history instead of behaving like dead UI.
It also verifies that paste input while scrolled back jumps to the live bottom.
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


ROWS = 24
COLS = 100
F12 = b"\x1b[24~"
PAGE_UP = b"\x1b[5~"
HOME = b"\x1b[H"
END = b"\x1b[F"
MOUSE_SCROLL_UP_AT_10_10 = b"\x1b[<64;10;10M"
LAST_LINE_SUFFIX = "LINE_160"
EARLY_LINE_SUFFIX = "LINE_001"
PASTE_BOTTOM_MARKER = "SCROLLBACK_PASTE_BOTTOM"


def build_parser() -> argparse.ArgumentParser:
    repo_root = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(
        description="Smoke-test tuimux active-terminal scrollback navigation."
    )
    parser.add_argument(
        "--binary",
        type=Path,
        default=repo_root / "target" / "debug" / "tuimux",
        help="tuimux binary to run. Default: target/debug/tuimux",
    )
    parser.add_argument(
        "--session",
        default=f"scrollback-smoke-{os.getpid()}",
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
        self.screen = AnsiScreen(ROWS, COLS)

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
            self.screen.feed(chunk)

    def clear_buffer(self) -> None:
        self.buffer.clear()

    def wait_screen_contains(self, text: str, timeout: float) -> bool:
        deadline = time.time() + timeout
        while time.time() < deadline:
            self.read_for(0.1)
            if self.screen.contains(text):
                return True
        return False

    def wait_screen_contains_any(self, texts: list[str], timeout: float) -> str | None:
        deadline = time.time() + timeout
        while time.time() < deadline:
            self.read_for(0.1)
            for text in texts:
                if self.screen.contains(text):
                    return text
        return None

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

    def screen_text(self) -> str:
        return self.screen.text()


class AnsiScreen:
    def __init__(self, rows: int, cols: int) -> None:
        self.rows = rows
        self.cols = cols
        self.cells = [[" " for _ in range(cols)] for _ in range(rows)]
        self.row = 0
        self.col = 0

    def feed(self, data: bytes) -> None:
        i = 0
        while i < len(data):
            byte = data[i]
            if byte == 0x1B:
                i = self._consume_escape(data, i)
                continue
            if byte == 0x0D:
                self.col = 0
            elif byte == 0x0A:
                self._line_feed()
            elif 0x20 <= byte < 0x7F:
                self._put(chr(byte))
            i += 1

    def contains(self, text: str) -> bool:
        return text in self.text()

    def text(self) -> str:
        return "\n".join("".join(row).rstrip() for row in self.cells)

    def _consume_escape(self, data: bytes, start: int) -> int:
        if start + 1 >= len(data):
            return start + 1
        if data[start + 1] != ord("["):
            return start + 2

        end = start + 2
        while end < len(data):
            byte = data[end]
            if 0x40 <= byte <= 0x7E:
                self._handle_csi(data[start + 2 : end].decode("ascii", "ignore"), chr(byte))
                return end + 1
            end += 1
        return len(data)

    def _handle_csi(self, params: str, final: str) -> None:
        clean = params.lstrip("?")
        parts = [part for part in clean.split(";") if part]
        if final in ("H", "f"):
            row = int(parts[0]) if len(parts) >= 1 and parts[0].isdigit() else 1
            col = int(parts[1]) if len(parts) >= 2 and parts[1].isdigit() else 1
            self.row = max(0, min(self.rows - 1, row - 1))
            self.col = max(0, min(self.cols - 1, col - 1))
        elif final == "J":
            mode = int(parts[0]) if parts and parts[0].isdigit() else 0
            if mode == 2:
                self._clear()
        elif final == "K":
            mode = int(parts[0]) if parts and parts[0].isdigit() else 0
            if mode == 0:
                for col in range(self.col, self.cols):
                    self.cells[self.row][col] = " "
            elif mode == 2:
                for col in range(self.cols):
                    self.cells[self.row][col] = " "

    def _put(self, char: str) -> None:
        if 0 <= self.row < self.rows and 0 <= self.col < self.cols:
            self.cells[self.row][self.col] = char
        self.col += 1
        if self.col >= self.cols:
            self.col = 0
            self._line_feed()

    def _line_feed(self) -> None:
        self.row += 1
        if self.row >= self.rows:
            self.cells.pop(0)
            self.cells.append([" " for _ in range(self.cols)])
            self.row = self.rows - 1

    def _clear(self) -> None:
        self.cells = [[" " for _ in range(self.cols)] for _ in range(self.rows)]
        self.row = 0
        self.col = 0


def stop_daemon(binary: Path, session: str) -> None:
    subprocess.run(
        [str(binary), "--stop-server", "--session", session],
        cwd=Path(__file__).resolve().parents[1],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False,
    )


def wait_screen_or_fail(client: PtyClient, text: str, timeout: float, label: str) -> None:
    if not client.wait_screen_contains(text, timeout):
        raise RuntimeError(
            f"{label} did not appear on screen; tail:\n{client.tail()}\n\nscreen:\n{client.screen_text()}"
        )


def wait_screen_any_or_fail(
    client: PtyClient, texts: list[str], timeout: float, label: str
) -> str:
    found = client.wait_screen_contains_any(texts, timeout)
    if found is None:
        raise RuntimeError(
            f"{label} did not appear on screen; expected one of {texts!r}; "
            f"tail:\n{client.tail()}\n\nscreen:\n{client.screen_text()}"
        )
    return found


def paste_shell(client: PtyClient, command: str) -> None:
    client.write(f"\x1b[200~{command}\x1b[201~".encode())
    client.read_for(0.2)
    client.write(b"\r")


def generate_scrollback(client: PtyClient, timeout: float) -> None:
    command = (
        "python3 -c 'import sys; "
        "sys.stdout.write(\"\\033[2J\\033[H\"); "
        "[print(f\"SCROLLBACK_LINE_{i:03d}\") for i in range(1, 161)]'"
    )
    paste_shell(client, command)
    wait_screen_or_fail(client, LAST_LINE_SUFFIX, timeout, "last scrollback line")


def enter_navigation(client: PtyClient, timeout: float) -> None:
    client.clear_buffer()
    client.write(F12)
    wait_screen_or_fail(client, "WINDOWS", timeout, "window list")
    wait_screen_or_fail(client, "+ new", timeout, "new-window row")


def verify_mouse_wheel_scrollback(client: PtyClient, timeout: float) -> str:
    client.clear_buffer()
    for _ in range(8):
        client.write(MOUSE_SCROLL_UP_AT_10_10)
        client.read_for(0.05)
    return wait_screen_any_or_fail(
        client,
        [f"LINE_{line:03d}" for line in range(126, 139)],
        timeout,
        "mouse wheel scrollback line",
    )


def verify_paste_returns_to_bottom(client: PtyClient, timeout: float) -> None:
    command = f"printf '\\033[2J\\033[H{PASTE_BOTTOM_MARKER}\\n'"
    client.clear_buffer()
    paste_shell(client, command)
    wait_screen_or_fail(
        client,
        PASTE_BOTTOM_MARKER,
        timeout,
        "paste while scrolled back did not return to the live bottom",
    )


def verify_page_home_end(client: PtyClient, timeout: float) -> None:
    enter_navigation(client, timeout)

    client.clear_buffer()
    client.write(PAGE_UP)
    wait_screen_any_or_fail(
        client,
        [f"LINE_{line:03d}" for line in range(90, 140)],
        timeout,
        "PageUp scrollback content",
    )

    client.clear_buffer()
    client.write(HOME)
    wait_screen_or_fail(client, EARLY_LINE_SUFFIX, timeout, "Home top scrollback content")

    client.clear_buffer()
    client.write(END)
    wait_screen_or_fail(client, "bottom", timeout, "End bottom status")
    wait_screen_any_or_fail(
        client,
        [f"LINE_{line:03d}" for line in range(150, 161)],
        timeout,
        "End bottom content",
    )


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
        client.read_for(1.5)
        generate_scrollback(client, args.timeout)
        wheel_line = verify_mouse_wheel_scrollback(client, args.timeout)
        verify_paste_returns_to_bottom(client, args.timeout)
        generate_scrollback(client, args.timeout)
        verify_page_home_end(client, args.timeout)

        print("OK macOS scrollback smoke")
        print(f"mouse wheel scrollback: observed {wheel_line}")
        print("paste while scrolled back: returned to live bottom")
        print("PageUp/Home/End scrollback navigation: observed")
        print("bottom restore: observed")
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
