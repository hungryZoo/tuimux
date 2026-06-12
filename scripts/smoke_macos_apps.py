#!/usr/bin/env python3
"""End-to-end macOS app smoke test for tuimux's native PTY surface."""

from __future__ import annotations

import argparse
import fcntl
import os
import pty
import select
import shlex
import shutil
import struct
import subprocess
import sys
import termios
import time
from pathlib import Path


ROWS = 32
COLS = 120
TIMEOUT = 10.0


def build_parser() -> argparse.ArgumentParser:
    repo_root = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(
        description="Smoke-test llmfit, btop, htop, and nano inside tuimux."
    )
    parser.add_argument(
        "--binary",
        type=Path,
        default=repo_root / "target" / "debug" / "tuimux",
        help="tuimux binary to run. Default: target/debug/tuimux",
    )
    parser.add_argument(
        "--session",
        default=f"app-smoke-{os.getpid()}",
        help="temporary tuimux session name",
    )
    parser.add_argument(
        "--timeout",
        type=float,
        default=TIMEOUT,
        help="seconds to wait for each app checkpoint",
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

    def wait_any(self, texts: tuple[str, ...], timeout: float) -> str | None:
        deadline = time.time() + timeout
        needles = [(text, text.encode()) for text in texts]
        while time.time() < deadline:
            self.read_for(0.1)
            for text, needle in needles:
                if needle in self.buffer:
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


def require_commands(commands: tuple[str, ...]) -> None:
    missing = [command for command in commands if shutil.which(command) is None]
    if missing:
        raise SystemExit(f"required command(s) not found: {', '.join(missing)}")


def stop_daemon(binary: Path, session: str) -> None:
    subprocess.run(
        [str(binary), "--stop-server", "--session", session],
        cwd=Path(__file__).resolve().parents[1],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False,
    )


def shell(client: PtyClient, command: str) -> None:
    client.write((command + "\r").encode())


def wait_or_fail(client: PtyClient, text: str, timeout: float, label: str) -> None:
    if not client.wait_contains(text, timeout):
        raise RuntimeError(f"{label} did not appear; tail:\n{client.tail()}")


def wait_file_contains(path: Path, text: str, timeout: float) -> None:
    deadline = time.time() + timeout
    while time.time() < deadline:
        if path.exists() and text in path.read_text(errors="replace"):
            return
        time.sleep(0.1)
    raise RuntimeError(f"{path} did not contain {text!r}")


def wait_process_absent(pattern: str, timeout: float) -> None:
    deadline = time.time() + timeout
    while time.time() < deadline:
        result = subprocess.run(
            ["pgrep", "-f", pattern],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
        )
        if result.returncode != 0:
            return
        time.sleep(0.1)
    raise RuntimeError(f"process matching {pattern!r} was still running")


def run_line_app(client: PtyClient, command: str, expected: str, marker: str, timeout: float) -> None:
    client.clear_buffer()
    shell(
        client,
        "printf '\\033[2J\\033[H'; "
        f"{command}; tuimux_status=$?; printf '\\n{marker}:%s\\n' \"$tuimux_status\"",
    )
    wait_or_fail(client, expected, timeout, f"{command} output")
    wait_or_fail(client, f"{marker}:0", timeout, f"{command} exit marker")


def wait_for_shell(client: PtyClient, timeout: float) -> None:
    marker = "TUIMUX_APP_SHELL_READY"
    client.clear_buffer()
    shell(client, f"printf '\\033[2J\\033[H{marker}\\n'")
    wait_or_fail(client, marker, timeout, "initial shell marker")


def run_fullscreen_app(
    client: PtyClient,
    command: str,
    startup_needles: tuple[str, ...],
    exit_key: bytes,
    marker: str,
    timeout: float,
) -> None:
    client.clear_buffer()
    shell(client, f"printf '\\033[2J\\033[H'; {command}")
    seen = client.wait_any(startup_needles, timeout)
    if seen is None:
        raise RuntimeError(f"{command} did not render expected UI; tail:\n{client.tail()}")
    client.write(exit_key)
    client.read_for(1.0)
    client.clear_buffer()
    shell(client, f"printf '\\033[2J\\033[H'; printf '{marker}:0\\n'")
    wait_or_fail(client, f"{marker}:0", timeout, f"{command} shell return")


def run_nano(client: PtyClient, timeout: float) -> None:
    target = Path("/tmp") / f"tuimux-nano-smoke-{os.getpid()}.txt"
    target.unlink(missing_ok=True)
    text = "tuimux nano smoke"

    client.clear_buffer()
    shell(client, f"printf '\\033[2J\\033[H'; nano {shlex.quote(str(target))}")
    seen = client.wait_any(
        ("GNU nano", "UW PICO", "File:", "WriteOut", "Exit", "Get", "Help"),
        timeout,
    )
    if seen is None:
        raise RuntimeError(f"nano did not render expected UI; tail:\n{client.tail()}")

    client.write(text.encode())
    client.read_for(0.2)
    client.write(b"\x0f")
    client.read_for(0.3)
    client.write(b"\r")
    wait_file_contains(target, text, timeout)
    client.read_for(0.5)
    client.write(b"\x18")
    wait_process_absent(str(target), timeout)
    target.unlink(missing_ok=True)


def main() -> int:
    parser = build_parser()
    args = parser.parse_args()
    binary = args.binary.resolve()
    if not binary.exists():
        parser.error(f"tuimux binary does not exist: {binary}")
    if sys.platform != "darwin":
        raise SystemExit("this smoke test currently targets macOS")
    require_commands(("llmfit", "btop", "htop", "nano"))

    client = PtyClient(binary, args.session)
    try:
        client.read_for(1.5)
        wait_for_shell(client, args.timeout)
        llmfit_capture = Path("/tmp") / f"tuimux-llmfit-help-{os.getpid()}.txt"
        run_line_app(
            client,
            "llmfit --help > "
            f"{shlex.quote(str(llmfit_capture))} 2>&1; "
            "app_status=$?; "
            f"sed -n '1,6p' {shlex.quote(str(llmfit_capture))}; "
            f"rm -f {shlex.quote(str(llmfit_capture))}; "
            'test "$app_status" -eq 0',
            "Right-size LLM models",
            "TUIMUX_APP_LLMFIT_DONE",
            args.timeout,
        )
        run_fullscreen_app(
            client,
            "btop",
            ("btop", "cpu", "CPU", "Mem", "Processes"),
            b"q",
            "TUIMUX_APP_BTOP_DONE",
            args.timeout,
        )
        run_fullscreen_app(
            client,
            "htop",
            ("htop", "F1Help", "F1", "Tasks", "Load average"),
            b"q",
            "TUIMUX_APP_HTOP_DONE",
            args.timeout,
        )
        run_nano(client, args.timeout)
        print("OK macOS app smoke")
        print("llmfit --help: rendered")
        print("btop: rendered and exited")
        print("htop: rendered and exited")
        print("nano: edited, saved, and exited")
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
