# tuimux

`tuimux` is an early Rust-native, prefix-free, mouse-first terminal multiplexer.

v0.2.0-alpha.35 keeps the default runtime on the Rust-native path, restores the
boxed Detach/WINDOWS/STATUS rail beside the live terminal, and fixes btop-style
cursor positioning in the terminal emulator without adding top or bottom status
bars. This prerelease keeps the OSC 52 clipboard loop from alpha.29 and
preserves the recent OSC title, scrollback, alternate-screen, resize, color,
selection, and child-exit checks.
Running `tuimux` attaches a ratatui client to tuimux's own Unix-socket daemon,
which owns one persistent window list and PTY-backed shell processes. `tmux` is
no longer required for the default UI; the old plain tmux client remains
available only through the hidden `--native-client` fallback.

See:

- [SRS](docs/srs.md)
- [SDD](docs/sdd.md)
- [PRD](docs/prd.md)
- [btop terminal emulator skill](docs/skills/btop-terminal-emulator.md)

## Current Prerelease Scope

This is still a 0.x prerelease. Current behavior:

- `tuimux` opens the Rust-native tuimux TUI by default.
- Terminal mode is now a full tuimux shell: wide terminals show a boxed right rail with Detach, WINDOWS, 3-cell ` X ` close buttons, `+ new`, and a Comet-colored STATUS panel that only shows `scroll:<count>` and jumps to bottom when clicked.
- Narrow terminals temporarily hide terminal-mode chrome instead of switching to compact top tabs, so apps such as `btop` can keep an honest 80-column PTY.
- The PTY parser normalizes HVP cursor-position sequences (`CSI row;col f`) used by full-screen apps such as `btop`.
- The child PTY uses only the terminal body outside the rail, so mouse and keyboard routing do not treat TUI controls as child terminal cells.
- `Alt-N` creates a window, and `Alt-Left`/`Alt-Right` switch windows while staying in terminal mode.
- Press `F12` to switch between terminal mode and navigation/sidebar mode.
- The window list and each active PTY-backed shell are managed by the tuimux daemon, not by tmux.
- Navigation mode shows a right-side window list; `Tab` and arrow keys move between windows, `n` creates a window, and `x` closes the active window.
- Child OSC 0/1/2 terminal titles are shown in the right-side window list, with static `shell` names kept as fallback.
- Split-pane UI, daemon protocol commands, and native mux split layout state are removed from the default product path. Use separate windows instead.
- Closing/detaching the UI keeps the daemon-owned PTYs alive for later reattach.
- Multiple clients can connect to the same daemon concurrently; they currently share active window state.
- Each window runs a real shell in a PTY, parsed with `vt100` and rendered with ratatui.
- Mouse wheel scrolls shell history when the child program is not using mouse tracking; `PageUp`/`PageDown`, `Home`, and `End` work in navigation mode, and paste while scrolled back returns to the live bottom, covered by the macOS scrollback smoke.
- In terminal mode, `Home`/`End` and macOS `Cmd-Shift-Left`/`Cmd-Shift-Right` move to the start/end of the active terminal input line.
- Mouse selection is visibly preserved after mouse-up and selected text is extracted by the daemon from the active PTY screen; macOS PTY smoke covers reverse-video selection highlight, drag + right-click context-menu Cut/Copy, Cut Backspace delivery for cursor-line selections, Backspace/delete/text replacement for editable selections, drag + Ctrl-C + `pbpaste`, context-menu Paste, child bracketed paste wrappers, and click-to-clear paste highlighting even when child mouse/application-cursor modes are active.
- `Ctrl-C`/`Ctrl-Shift-C`/`Cmd-Shift-C` copy only when a drag selection exists; `Ctrl-Shift-X`/`Cmd-Shift-X` cut current input-line selections by copying then moving the cursor and sending Backspace, and otherwise fall back to copy + selection clear for non-editable screen text. Backspace/Delete consume editable selections, and normal text input replaces them. Without a selection, plain `Ctrl-C` still reaches the child program as SIGINT. `Ctrl-V`/`Ctrl-Shift-V`/`Cmd-Shift-V` paste the system clipboard into the active PTY.
- Child OSC 52 clipboard copy requests are decoded into the macOS system clipboard, and OSC 52 paste queries receive a base64 clipboard response.
- Host paste is handled as either a paste event or raw bracketed-paste key sequence; the next normal mouse click, including right click and down/up variants, clears shell-side paste highlighting with raw `ESC[D ESC[C` before context menus handle the click.
- If the child program enables mouse tracking, simple left clicks and wheel events still go to the child, while a normal drag starts tuimux text selection, covered by the macOS mouse-protocol smoke.
- Child truecolor foreground/background and default-color reset are preserved by the real TUI renderer, covered by the macOS color smoke.
- Host resize reaches the active child PTY as a SIGWINCH with updated rows/columns, covered by the macOS resize smoke.
- Alternate-screen output is visible while active, primary screen rendering is restored after exit, and alternate-screen text is kept out of primary scrollback.
- If a child shell exits, its final screen is exposed for one snapshot, then non-last windows are pruned and the last window is replaced with a fresh shell, covered by the macOS child-exit smoke.
- `tuimux --native-client` is a fallback that opens a plain tmux client when tmux is installed.
- `tuimux --doctor`, `--version`, and `--layout-preview` remain available.

Important alpha limitation: daemon state is in-memory only. Windows survive UI
detach/reattach, but not daemon shutdown, reboot, or `tuimux --stop-server`.

## Install

The current prerelease publishes macOS Apple Silicon only.

```sh
curl -fsSL https://raw.githubusercontent.com/hungryZoo/tuimux/v0.2.0-alpha.35/scripts/install.sh | \
  TUIMUX_VERSION=v0.2.0-alpha.35 bash
```

Verify:

```sh
tuimux --version
tuimux --doctor
tuimux
```

## Build From Source

```sh
cargo build --release
./target/release/tuimux --doctor
```

## Development

```sh
cargo fmt -- --check
cargo test
cargo run -- --layout-preview
python3 scripts/smoke_macos_terminal_chrome.py --binary target/debug/tuimux
python3 scripts/smoke_macos_ui_selection.py --binary target/debug/tuimux
python3 scripts/smoke_macos_apps.py --binary target/debug/tuimux
python3 scripts/smoke_macos_mouse_protocol.py --binary target/debug/tuimux
python3 scripts/smoke_macos_scrollback.py --binary target/debug/tuimux
python3 scripts/smoke_macos_color.py --binary target/debug/tuimux
python3 scripts/smoke_macos_resize.py --binary target/debug/tuimux
python3 scripts/smoke_macos_altscreen.py --binary target/debug/tuimux
python3 scripts/smoke_macos_window_title.py --binary target/debug/tuimux
python3 scripts/smoke_macos_osc52_clipboard.py --binary target/debug/tuimux
python3 scripts/smoke_macos_osc52_paste.py --binary target/debug/tuimux
python3 scripts/smoke_macos_window_flow.py --binary target/debug/tuimux
python3 scripts/smoke_macos_child_exit.py --binary target/debug/tuimux
python3 scripts/smoke_macos_no_tmux.py --binary target/debug/tuimux
```

## Release

Pushing a tag like `v0.2.0-alpha.35` triggers `.github/workflows/release.yml`,
which currently publishes a GitHub prerelease for macOS Apple Silicon only.
