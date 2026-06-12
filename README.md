# tuimux

`tuimux` is an early Rust-native, prefix-free, mouse-first terminal multiplexer.

v0.2.0-alpha.21 keeps the default runtime on the Rust-native path and focuses
the product on a single full-size terminal surface selected from a window list.
Running `tuimux` attaches a ratatui client to tuimux's own Unix-socket daemon,
which owns sessions, windows, and PTY-backed shell processes. `tmux` is
no longer required for the default UI; the old plain tmux client remains
available only through the hidden `--native-client` fallback.

See:

- [SRS](docs/srs.md)
- [SDD](docs/sdd.md)
- [PRD](docs/prd.md)

## Current Prerelease Scope

This is still a 0.x prerelease. Current behavior:

- `tuimux` opens the Rust-native tuimux TUI by default.
- Terminal mode is full-screen so full-screen tools receive the real host size.
- Press `F12` to switch between terminal mode and navigation/sidebar mode.
- Sessions, windows, and each active PTY-backed shell are managed by the tuimux daemon, not by tmux.
- Navigation mode shows a right-side window list; `Tab` and arrow keys move between windows, `n` creates a window, and `x` closes the active window.
- Split-pane UI, daemon protocol commands, and native mux split layout state are removed from the default product path. Use separate windows instead.
- Closing/detaching the UI keeps the daemon-owned PTYs alive for later reattach.
- Multiple clients can connect to the same daemon concurrently; they currently share active session/window state.
- Each window runs a real shell in a PTY, parsed with `vt100` and rendered with ratatui.
- Mouse wheel scrolls shell history when the child program is not using mouse tracking; `PageUp`/`PageDown`, `Home`, and `End` work in navigation mode.
- Mouse selection is visibly preserved after mouse-up and selected text is extracted by the daemon from the active PTY screen; macOS PTY smoke covers reverse-video selection highlight, drag + Ctrl-C + `pbpaste`, host bracketed paste, and child bracketed paste wrappers.
- Ctrl-C copies the selected text to the system clipboard instead of sending SIGINT.
- Host paste is captured as a paste event and forwarded to the active PTY with child bracketed paste respected.
- If the child program enables mouse tracking, normal mouse events go to the child; Shift-drag starts tuimux text selection, covered by the macOS mouse-protocol smoke.
- `tuimux --native-client` is a fallback that opens a plain tmux client when tmux is installed.
- `tuimux --doctor`, `--version`, and `--layout-preview` remain available.

Important alpha limitation: daemon state is in-memory only. Sessions survive UI
detach/reattach, but not daemon shutdown, reboot, or `tuimux --stop-server`.

## Install

The current prerelease publishes macOS Apple Silicon only.

```sh
curl -fsSL https://raw.githubusercontent.com/hungryZoo/tuimux/v0.2.0-alpha.21/scripts/install.sh | \
  TUIMUX_VERSION=v0.2.0-alpha.21 bash
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
python3 scripts/smoke_macos_ui_selection.py --binary target/debug/tuimux
python3 scripts/smoke_macos_apps.py --binary target/debug/tuimux
python3 scripts/smoke_macos_mouse_protocol.py --binary target/debug/tuimux
python3 scripts/smoke_macos_session_flow.py --binary target/debug/tuimux
python3 scripts/smoke_macos_no_tmux.py --binary target/debug/tuimux
```

## Release

Pushing a tag like `v0.2.0-alpha.21` triggers `.github/workflows/release.yml`,
which currently publishes a GitHub prerelease for macOS Apple Silicon only.
