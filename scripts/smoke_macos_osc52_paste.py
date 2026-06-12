#!/usr/bin/env python3
"""End-to-end macOS smoke test for child OSC 52 clipboard paste query."""

from __future__ import annotations

import argparse
import fcntl
import os
import pty
import shlex
import select
import struct
import subprocess
import sys
import termios
import tempfile
import textwrap
import time
from pathlib import Path


ROWS = 24
COLS = 100
CLIP_TEXT = "OSC52_PASTE_OK"
QUERY_SCRIPT = r"""
import base64
import os
import re
import select
import sys
import termios
import time
import tty

fd = sys.stdin.fileno()
old = termios.tcgetattr(fd)
data = b""
try:
    tty.setraw(fd)
    sys.stdout.write("\033]52;c;?\a")
    sys.stdout.flush()
    deadline = time.time() + 3.0
    while time.time() < deadline:
        ready, _, _ = select.select([fd], [], [], 0.1)
        if not ready:
            continue
        chunk = os.read(fd, 4096)
        if not chunk:
            break
        data += chunk
        if b"\a" in data or b"\033\\" in data:
            break
finally:
    termios.tcsetattr(fd, termios.TCSADRAIN, old)

match = re.search(br"\033]52;[^;]*;([A-Za-z0-9+/=]+)(?:\a|\033\\)", data)
if not match:
    print("OSC52_PASTE_MISSING:" + repr(data))
else:
    decoded = base64.b64decode(match.group(1)).decode("utf-8", "replace")
    print("OSC52_PASTE:" + decoded)
"""


def build_parser() -> argparse.ArgumentParser:
    repo_root = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(
        description="Smoke-test child OSC 52 paste query from the macOS clipboard."
    )
    parser.add_argument(
        "--binary",
        type=Path,
        default=repo_root / "target" / "debug" / "tuimux",
        help="tuimux binary to run. Default: target/debug/tuimux",
    )
    parser.add_argument(
        "--session",
        default=f"osc52-paste-smoke-{os.getpid()}",
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


def wait_or_fail(client: PtyClient, text: str, timeout: float, label: str) -> None:
    if not client.wait_contains(text, timeout):
        raise RuntimeError(f"{label} did not appear; tail:\n{client.tail()}")


def paste_shell(client: PtyClient, command: str) -> None:
    client.write(f"\x1b[200~{command}\x1b[201~".encode())
    client.read_for(0.2)
    client.write(b"\r")


def write_query_script() -> Path:
    script = tempfile.NamedTemporaryFile(
        "w",
        encoding="utf-8",
        prefix="tuimux-osc52-query-",
        suffix=".py",
        delete=False,
    )
    with script:
        script.write(textwrap.dedent(QUERY_SCRIPT).lstrip())
    return Path(script.name)


def osc52_query_command(script: Path) -> str:
    return f"python3 {shlex.quote(str(script))}"


def main() -> int:
    parser = build_parser()
    args = parser.parse_args()
    binary = args.binary.resolve()
    if not binary.exists():
        parser.error(f"tuimux binary does not exist: {binary}")
    if sys.platform != "darwin":
        raise SystemExit("this smoke test currently targets macOS")

    client = PtyClient(binary, args.session)
    query_script = write_query_script()
    try:
        set_clipboard(CLIP_TEXT)
        client.read_for(1.5)
        paste_shell(client, osc52_query_command(query_script))
        wait_or_fail(
            client,
            f"OSC52_PASTE:{CLIP_TEXT}",
            args.timeout,
            "OSC 52 paste response",
        )

        print("OK macOS OSC 52 paste smoke")
        print(f"clipboard query: {CLIP_TEXT}")
        return 0
    except RuntimeError as exc:
        print(exc, file=sys.stderr)
        return 1
    finally:
        client.close()
        try:
            query_script.unlink()
        except OSError:
            pass
        if not args.keep_daemon:
            stop_daemon(binary, args.session)


if __name__ == "__main__":
    raise SystemExit(main())
