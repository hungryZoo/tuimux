# tuimux

`tuimux` is an early Rust-native, prefix-free, mouse-first terminal multiplexer.

v0.2.0-alpha.39 keeps the default runtime on the Rust-native path, restores the
boxed Detach/WINDOWS/STATUS rail beside the live terminal, fixes btop-style
cursor positioning, and tightens terminal interaction so copy, cut, paste,
click, drag, and right-click menu paths behave consistently. Editable
selections now behave more like a text editor: soft-wrapped rows and hard
multi-line selections are deleted before Backspace/Delete/text/Cut/Paste
replacement while blank drag tails are ignored. This prerelease keeps the OSC
52 clipboard loop from alpha.29 and preserves the recent OSC title, scrollback,
alternate-screen, resize, color, selection, and child-exit checks.
Running `tuimux` attaches a ratatui client to tuimux's own Unix-socket daemon,
which owns one persistent window list and PTY-backed shell processes. `tmux` is
no longer required for the default UI; the old plain tmux client remains
available only through the hidden `--native-client` fallback.

See:

- [SRS](docs/srs.md)
- [SDD](docs/sdd.md)
- [PRD](docs/prd.md)
- [btop terminal emulator skill](docs/skills/btop-terminal-emulator.md)
- [terminal copy/cut/paste interaction skill](docs/skills/terminal-copy-cut-paste-interaction.md)

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
- In terminal mode, `Home`/`End`, `Cmd-Shift-Left`/`Cmd-Shift-Right`, and `Win-Shift-Left`/`Win-Shift-Right` move to the start/end of the active terminal input line when the host terminal forwards those modifier keys.
- When the active input cursor is visible, plain `Shift+Arrow` extends a text-editor-like selection from the cursor; `Shift+Up`/`Shift+Down` move by one visual row without triggering shell history.
- Mouse selection is visibly preserved after mouse-up and selected text is extracted by the daemon from the active PTY screen. Left click without drag moves the active input cursor to the clicked cell; drag creates selection; right-click opens the TUI Cut/Copy/Paste/Cancel menu.
- macOS PTY smoke covers reverse-video selection highlight, drag + right-click context-menu Cut/Copy, Cut Backspace delivery for editable selections, Shift-Left keyboard selection deletion, hard multi-line Backspace deletion, soft-wrap text replacement, Backspace/delete/text/Ctrl-V replacement for editable selections, drag + Ctrl-C + `pbpaste`, context-menu Paste, Ctrl-V Paste, and Super-Shift-V Paste execution, child bracketed paste wrappers, paste cursor-move clearing, and a real zsh visual check for final cursor column.
- `Ctrl-C`/`Ctrl-Shift-C`/`Cmd-Shift-C`/`Win-Shift-C` copy when a mouse or keyboard selection exists; `Ctrl-Shift-X`/`Cmd-Shift-X`/`Win-Shift-X` cut editable selections by copying then moving the cursor and sending Backspace, and otherwise fall back to copy + selection clear for non-editable screen text. Backspace/Delete consume editable selections, including soft-wrapped and hard multi-line selections, and normal text or Ctrl-V paste replaces them. Without a selection, plain `Ctrl-C` still reaches the child program as SIGINT. `Ctrl-V`/`Ctrl-Shift-V`/`Cmd-Shift-V`/`Win-Shift-V` paste the system clipboard into the active PTY through the same paste path as context-menu Paste.
- Child OSC 52 clipboard copy requests are decoded into the macOS system clipboard, and OSC 52 paste queries receive a base64 clipboard response.
- Host paste is handled as either a paste event or raw bracketed-paste key sequence; the next terminal-body left click moves the active input cursor to the clicked cell, which clears shell-side paste highlighting without leaking mouse bytes to the child.
- If the child program enables mouse tracking, simple left clicks and wheel events still go to the child, while a normal drag starts tuimux text selection, covered by the macOS mouse-protocol smoke.
- Child truecolor foreground/background and default-color reset are preserved by the real TUI renderer, covered by the macOS color smoke.
- Host resize reaches the active child PTY as a SIGWINCH with updated rows/columns, covered by the macOS resize smoke.
- Alternate-screen output is visible while active, primary screen rendering is restored after exit, and alternate-screen text is kept out of primary scrollback.
- If a child shell exits, its final screen is exposed for one snapshot, then non-last windows are pruned and the last window is replaced with a fresh shell, covered by the macOS child-exit smoke.
- `tuimux --native-client` is a fallback that opens a plain tmux client when tmux is installed.
- `tuimux --doctor`, `--version`, and `--layout-preview` remain available.

Important alpha limitation: daemon state is in-memory only. Windows survive UI
detach/reattach, but not daemon shutdown, reboot, or `tuimux --stop-server`.

## Keyboard Shortcuts

`Cmd` and `Win` below are treated as the same “launcher” modifier family inside tuimux. Crossterm can receive that family as `SUPER`, `META`, or `HYPER`, and tuimux enables keyboard-enhancement parsing so terminals that support it can forward those keys even when tuimux is running on a remote server.

If your local terminal reserves a shortcut and never sends it to the remote PTY, tuimux cannot see it. In that case use the `Ctrl-Shift-*` variant or the right-click menu.

| Context | Shortcut | Action |
| --- | --- | --- |
| Global | `F12` | Toggle terminal mode and navigation/sidebar mode. |
| Terminal mode | `Alt-N` | Create a new window. |
| Terminal mode | `Alt-Left` / `Alt-Right` | Switch active window. |
| Terminal input | `Home` / `End` | Move to start/end of the active input line. |
| Terminal input | `Cmd-Shift-Left` / `Cmd-Shift-Right` | Same as `Home` / `End`. |
| Terminal input | `Win-Shift-Left` / `Win-Shift-Right` | Same as `Home` / `End`. |
| Terminal input | `Shift-Left` / `Shift-Right` | Extend keyboard selection by one character. |
| Terminal input | `Shift-Up` / `Shift-Down` | Extend keyboard selection by one visual row without triggering shell history. |
| Selection | `Ctrl-C` | Copy selection; without selection, plain `Ctrl-C` is sent to the child process. |
| Selection | `Ctrl-Shift-C` / `Cmd-Shift-C` / `Win-Shift-C` | Copy selection. Without selection, the launcher variants are consumed instead of typing `c`. |
| Editable selection | `Ctrl-Shift-X` / `Cmd-Shift-X` / `Win-Shift-X` | Cut selection: copy to system clipboard, then delete editable text with cursor movement and Backspace. |
| Paste | `Ctrl-V` / `Ctrl-Shift-V` / `Cmd-Shift-V` / `Win-Shift-V` | Paste system clipboard into the active PTY; editable selections are replaced first. |
| Mouse | left click | Move active input cursor to the clicked terminal cell, or clear an existing selection. |
| Mouse | left drag | Create a text selection. |
| Mouse | right click | Open the Cut / Copy / Paste / Cancel context menu. |
| Navigation mode | `Tab` / arrow keys | Move between windows. |
| Navigation mode | `n` | Create a new window. |
| Navigation mode | `x` | Close the active window. |
| Navigation mode | `PageUp` / `PageDown` / `Home` / `End` | Navigate scrollback. |
| Navigation mode | `q` / `Esc` / `Ctrl-C` | Exit the UI client. |

## Install

The current prerelease publishes macOS Apple Silicon only.

```sh
curl -fsSL https://raw.githubusercontent.com/hungryZoo/tuimux/v0.2.0-alpha.39/scripts/install.sh | \
  TUIMUX_VERSION=v0.2.0-alpha.39 bash
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

Pushing a tag like `v0.2.0-alpha.39` triggers `.github/workflows/release.yml`,
which currently publishes a GitHub prerelease for macOS Apple Silicon only.
