# tuimux

`tuimux` is an early Rust-native, prefix-free, mouse-first terminal multiplexer.

v0.2.0-alpha.5 changes the default runtime from “tmux-backed UI” to an embedded
Rust multiplexer. Running `tuimux` now starts tuimux's own in-process sessions
and PTY-backed shell windows. `tmux` is no longer required for the default UI;
the old plain tmux client remains available only through the hidden
`--native-client` fallback.

See:

- [SRS](docs/srs.md)
- [SDD](docs/sdd.md)
- [PRD](docs/prd.md)

## Current Prerelease Scope

This is still a 0.x prerelease. Current behavior:

- `tuimux` opens the Rust-native tuimux TUI by default.
- Terminal mode is full-screen so full-screen tools receive the real host size.
- Press `F12` to switch between terminal mode and navigation/sidebar mode.
- Sessions and windows are managed inside the tuimux process.
- Each window runs a real shell in a PTY, parsed with `vt100` and rendered with ratatui.
- Mouse selection is preserved after mouse-up.
- Ctrl-C copies the selected text to the system clipboard instead of sending SIGINT.
- If the child program enables mouse tracking, normal mouse events go to the child; Shift-drag starts tuimux text selection.
- `tuimux --native-client` is a fallback that opens a plain tmux client when tmux is installed.
- `tuimux --doctor`, `--version`, and `--layout-preview` remain available.

Important alpha limitation: sessions are currently in-process. Closing or
detaching the UI also ends the child PTYs; a persistent tmux-style daemon/server
is the next large backend step.

## Install

The current prerelease publishes macOS Apple Silicon only.

```sh
curl -fsSL https://raw.githubusercontent.com/hungryZoo/tuimux/v0.2.0-alpha.5/scripts/install.sh | \
  TUIMUX_VERSION=v0.2.0-alpha.5 bash
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
```

## Release

Pushing a tag like `v0.2.0-alpha.5` triggers `.github/workflows/release.yml`,
which currently publishes a GitHub prerelease for macOS Apple Silicon only.
