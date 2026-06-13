#!/usr/bin/env python3
"""End-to-end macOS smoke test for tuimux clipboard behavior.

The test starts the real tuimux TUI in a pseudo terminal, prints a stable target
string in the child shell, selects it with SGR mouse escape sequences, presses
right-click and Ctrl-C, and verifies that macOS `pbpaste` contains the selected
text. Before copying, it also checks that the mouse-up frame renders the
selected text with reverse-video highlighting so the selection visibly persists.
A shell trap also confirms that Ctrl-C was not forwarded to the foreground
child. It then verifies the right-click context menu's Copy and Paste actions
against the system clipboard. Finally, a raw child process enables bracketed
paste mode and verifies that tuimux preserves the child-side paste wrapper.
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


MARKER = "TUIMUX_UI_COPY_TARGET"
RIGHT_PASTE_MARKER = "TUIMUX_UI_RIGHT_PASTE_TARGET"
RIGHT_PASTE_OUTPUT = f"RIGHT_PASTE_RAN:{RIGHT_PASTE_MARKER}"
CHILD_PASTE_MARKER = "TUIMUX_CHILD_BRACKETED_TARGET"
CHILD_PASTE_READY = "CHILD_BRACKETED_READY"
CHILD_PASTE_OK = "CHILD_BRACKETED_OK"
PASTE_CLICK_MARKER = "TUIMUX_PASTE_CLICK_CLEAR_TARGET"
PASTE_CLICK_READY = "PASTE_CLICK_CLEAR_READY"
PASTE_CLICK_WAITING = "PASTE_CLICK_CLEAR_WAITING"
PASTE_CLICK_OK = "PASTE_CLICK_CLEAR_OK"
SENTINEL = "TUIMUX_CLIPBOARD_SENTINEL"
ROWS = 24
COLS = 100


def build_parser() -> argparse.ArgumentParser:
    repo_root = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(
        description="Smoke-test tuimux mouse selection, copy, and paste on macOS."
    )
    parser.add_argument(
        "--binary",
        type=Path,
        default=repo_root / "target" / "debug" / "tuimux",
        help="tuimux binary to run. Default: target/debug/tuimux",
    )
    parser.add_argument(
        "--session",
        default=f"ui-selection-smoke-{os.getpid()}",
        help="temporary tuimux session name",
    )
    parser.add_argument(
        "--timeout",
        type=float,
        default=8.0,
        help="seconds to wait for the marker to appear",
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
        return bytes(self.buffer[-4000:]).decode("utf-8", "replace")


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


def child_bracketed_paste_probe_command(payload: str, script_path: Path) -> str:
    probe = f"""
import os
import select
import sys
import termios
import time
import tty

expected = b"\\x1b[200~" + {payload.encode()!r} + b"\\x1b[201~"
fd = sys.stdin.fileno()
old = termios.tcgetattr(fd)
try:
    tty.setraw(fd)
    os.write(sys.stdout.fileno(), b"\\x1b[?2004h{CHILD_PASTE_READY}\\r\\n")
    data = b""
    deadline = time.time() + 5.0
    while time.time() < deadline and b"\\x1b[201~" not in data:
        ready, _, _ = select.select([fd], [], [], 0.1)
        if ready:
            chunk = os.read(fd, 1024)
            if not chunk:
                break
            data += chunk
    os.write(sys.stdout.fileno(), b"\\x1b[?2004l")
    if data == expected:
        os.write(sys.stdout.fileno(), b"{CHILD_PASTE_OK}\\r\\n")
    else:
        os.write(sys.stdout.fileno(), b"CHILD_BRACKETED_BAD:" + data.hex().encode() + b"\\r\\n")
finally:
    termios.tcsetattr(fd, termios.TCSADRAIN, old)
"""
    script_path.write_text(probe, encoding="utf-8")
    return f"python3 {shlex.quote(str(script_path))}"


def paste_click_clear_probe_command(payload: str, script_path: Path) -> str:
    probe = f"""
import os
import select
import sys
import termios
import time
import tty

expected = b"\\x1b[200~" + {payload.encode()!r} + b"\\x1b[201~"
fd = sys.stdin.fileno()
old = termios.tcgetattr(fd)
try:
    tty.setraw(fd)
    os.write(sys.stdout.fileno(), b"\\x1b[?2004h{PASTE_CLICK_READY}\\r\\n")
    data = b""
    deadline = time.time() + 5.0
    while time.time() < deadline and b"\\x1b[201~" not in data:
        ready, _, _ = select.select([fd], [], [], 0.1)
        if ready:
            chunk = os.read(fd, 1024)
            if not chunk:
                break
            data += chunk
    if data != expected:
        os.write(sys.stdout.fileno(), b"PASTE_CLICK_BAD_PASTE:" + data.hex().encode() + b"\\r\\n")
    else:
        os.write(sys.stdout.fileno(), b"\\x1b[47;30m" + {payload.encode()!r} + b"\\x1b[0m\\r\\n{PASTE_CLICK_WAITING}\\r\\n")
        click_data = b""
        deadline = time.time() + 5.0
        while (
            time.time() < deadline
            and b"\\x1b[C" not in click_data
            and b"\\x1bOC" not in click_data
        ):
            ready, _, _ = select.select([fd], [], [], 0.1)
            if ready:
                chunk = os.read(fd, 1024)
                if not chunk:
                    break
                click_data += chunk
        if b"\\x1b[C" in click_data or b"\\x1bOC" in click_data:
            os.write(sys.stdout.fileno(), b"\\x1b[?2004l{PASTE_CLICK_OK}\\r\\n")
        else:
            os.write(sys.stdout.fileno(), b"\\x1b[?2004lPASTE_CLICK_BAD_CLEAR:" + click_data.hex().encode() + b"\\r\\n")
finally:
    termios.tcsetattr(fd, termios.TCSADRAIN, old)
"""
    script_path.write_text(probe, encoding="utf-8")
    return f"python3 {shlex.quote(str(script_path))}"


def has_reverse_video_highlight(output: bytes, text: str) -> bool:
    text_bytes = re.escape(text.encode())
    return (
        re.search(rb"\x1b\[(?:[0-9;]*;)?7(?:;[0-9]*)?m" + text_bytes, output)
        is not None
    )


def main() -> int:
    parser = build_parser()
    args = parser.parse_args()
    binary = args.binary.resolve()
    if not binary.exists():
        parser.error(f"tuimux binary does not exist: {binary}")
    require_macos()

    trap_file = Path("/tmp") / f"tuimux-ui-copy-int-{os.getpid()}"
    child_paste_script = Path("/tmp") / f"tuimux-child-bracketed-{os.getpid()}.py"
    paste_click_script = Path("/tmp") / f"tuimux-paste-click-clear-{os.getpid()}.py"
    trap_file.unlink(missing_ok=True)
    child_paste_script.unlink(missing_ok=True)
    paste_click_script.unlink(missing_ok=True)
    set_clipboard(SENTINEL)

    client = PtyClient(binary, args.session)
    try:
        client.read_for(1.5)
        if not client.wait_contains("WINDOWS", args.timeout):
            print("tuimux UI did not become ready", file=sys.stderr)
            print(client.tail(), file=sys.stderr)
            return 1
        command = (
            f"rm -f {trap_file}; "
            "printf '\\033[2J\\033[H'; "
            f"sh -c \"trap 'printf INT > {trap_file}' INT; "
            f"printf '{MARKER}'; sleep 4\"\r"
        )
        client.write(command.encode())
        if not client.wait_contains(MARKER, args.timeout):
            print("marker did not appear in tuimux output", file=sys.stderr)
            print(client.tail(), file=sys.stderr)
            return 1
        client.read_for(0.6)

        # crossterm enables SGR mouse mode. Coordinates are 1-based.
        y = 1
        x1 = 1
        x2 = len(MARKER)
        drag = f"\x1b[<0;{x1};{y}M\x1b[<32;{x2};{y}M\x1b[<0;{x2};{y}m"
        before_drag = len(client.buffer)
        client.write(drag.encode())
        client.read_for(0.6)
        highlighted_frame = bytes(client.buffer[before_drag:])
        if not has_reverse_video_highlight(highlighted_frame, MARKER):
            print("selection did not remain visibly highlighted after mouse-up", file=sys.stderr)
            print(client.tail(), file=sys.stderr)
            return 1

        right_click_menu = f"\x1b[<2;{x2};{y}M\x1b[<2;{x2};{y}m"
        client.write(right_click_menu.encode())
        if not client.wait_contains("Copy", args.timeout):
            print("right-click context menu did not appear", file=sys.stderr)
            print(client.tail(), file=sys.stderr)
            return 1

        copy_item_click = f"\x1b[<0;{x2 + 2};{y + 1}M\x1b[<0;{x2 + 2};{y + 1}m"
        client.write(copy_item_click.encode())
        time.sleep(0.5)

        right_copied = get_clipboard()
        if right_copied != MARKER:
            print(
                f"right-click clipboard mismatch: expected {MARKER!r}, got {right_copied!r}",
                file=sys.stderr,
            )
            return 1

        client.write(b"\x03")
        time.sleep(0.5)

        copied = get_clipboard()
        if copied != MARKER:
            print(f"clipboard mismatch: expected {MARKER!r}, got {copied!r}", file=sys.stderr)
            return 1
        if trap_file.exists():
            print(
                "Ctrl-C reached the foreground child instead of copying selection",
                file=sys.stderr,
            )
            return 1

        time.sleep(3.0)
        clear_selection_click = "\x1b[<0;1;1M\x1b[<0;1;1m"
        client.write(clear_selection_click.encode())
        client.read_for(0.2)

        right_paste_payload = (
            "printf '\\033[2J\\033[H'; "
            f"printf 'RIGHT_PASTE_RAN:%s\\n' {RIGHT_PASTE_MARKER}"
            "\n"
        )
        set_clipboard(right_paste_payload)
        client.write(b"\x1b[<2;1;1M\x1b[<2;1;1m")
        if not client.wait_contains("Paste", args.timeout):
            print("right-click paste menu did not appear", file=sys.stderr)
            print(client.tail(), file=sys.stderr)
            return 1
        client.write(b"\x1b[<0;3;3M\x1b[<0;3;3m")
        client.read_for(0.2)
        client.write(b"\r")
        if not client.wait_contains(RIGHT_PASTE_OUTPUT, args.timeout):
            print("right-click pasted command output did not appear", file=sys.stderr)
            print(client.tail(), file=sys.stderr)
            return 1

        child_probe_command = child_bracketed_paste_probe_command(
            CHILD_PASTE_MARKER, child_paste_script
        )
        client.write((child_probe_command + "\r").encode())
        if not client.wait_contains(CHILD_PASTE_READY, args.timeout):
            print("child bracketed paste probe did not become ready", file=sys.stderr)
            print(client.tail(), file=sys.stderr)
            return 1

        child_bracketed_paste = f"\x1b[200~{CHILD_PASTE_MARKER}\x1b[201~"
        client.write(child_bracketed_paste.encode())
        if not client.wait_contains(CHILD_PASTE_OK, args.timeout):
            print("child did not receive bracketed paste wrapper from tuimux", file=sys.stderr)
            print(client.tail(), file=sys.stderr)
            return 1

        paste_click_command = paste_click_clear_probe_command(
            PASTE_CLICK_MARKER, paste_click_script
        )
        client.write((paste_click_command + "\r").encode())
        if not client.wait_contains(PASTE_CLICK_READY, args.timeout):
            print("paste click clear probe did not become ready", file=sys.stderr)
            print(client.tail(), file=sys.stderr)
            return 1
        paste_click_payload = f"\x1b[200~{PASTE_CLICK_MARKER}\x1b[201~"
        client.write(paste_click_payload.encode())
        if not client.wait_contains(PASTE_CLICK_WAITING, args.timeout):
            print("paste click clear probe did not receive bracketed paste", file=sys.stderr)
            print(client.tail(), file=sys.stderr)
            return 1
        client.write(b"\x1b[<0;1;1M\x1b[<0;1;1m")
        if not client.wait_contains(PASTE_CLICK_OK, args.timeout):
            print("terminal click did not clear paste highlight state", file=sys.stderr)
            print(client.tail(), file=sys.stderr)
            return 1

        print("OK macOS UI selection smoke")
        print("selection highlight: reverse video observed after mouse-up")
        print(f"right-click menu copied: {right_copied}")
        print(f"Ctrl-C copied: {copied}")
        print(f"right-click menu pasted command output: {RIGHT_PASTE_OUTPUT}")
        print("child bracketed paste wrapper: observed")
        print("paste highlight click clear: observed")
        print("foreground child SIGINT: not observed")
        return 0
    finally:
        client.close()
        trap_file.unlink(missing_ok=True)
        child_paste_script.unlink(missing_ok=True)
        paste_click_script.unlink(missing_ok=True)
        if not args.keep_daemon:
            stop_daemon(binary, args.session)


if __name__ == "__main__":
    raise SystemExit(main())
