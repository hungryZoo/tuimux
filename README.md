# tuimux

`tuimux` is an early Rust MVP for a VS Code-inspired, mouse-first TUI front-end for tmux.

The long-term goal is to use `tmux` as the backend/session engine and provide a simpler full-TUI wrapper:

- center tmux pane area
- compact right sidebar with a `Session` button, red **Detach** button, and vertical window tabs
- centered, headerless session dialog for switching sessions or detaching
- no tmux prefix-key workflow for normal operations

See:

- [PRD](docs/prd.md)
- [SRS](docs/srs.md)

## Current prerelease scope

This is still a 0.x prerelease. The current binary now includes a functional tmux-backed TUI:

- `tuimux --help`
- `tuimux --version`
- `tuimux --doctor` to check tmux and terminal readiness
- `tuimux --layout-preview` to render the planned compact VS Code-like layout
- a safe interactive TUI shell with no top header row, real tmux session/window controls, `New Session`, window close `✕`, and a real tmux pane rendered from visible-screen `capture-pane` with keyboard input forwarded by `send-keys` after clicking the pane (`F12` returns to navigation mode). Capture is visible-screen-only, so `clear` and full-screen programs such as `nano` follow tmux semantics.

## macOS install

Runtime dependency:

```sh
brew install tmux
```

### One-line installer

```sh
curl -fsSL https://raw.githubusercontent.com/hungryZoo/tuimux/main/scripts/install.sh | bash
```

Install a specific prerelease tag:

```sh
TUIMUX_VERSION=v0.1.6 \
  curl -fsSL https://raw.githubusercontent.com/hungryZoo/tuimux/main/scripts/install.sh | bash
```

Custom install directory:

```sh
TUIMUX_INSTALL_DIR="$HOME/bin" \
  curl -fsSL https://raw.githubusercontent.com/hungryZoo/tuimux/main/scripts/install.sh | bash
```

Verify:

```sh
tuimux --version
tuimux --doctor
tuimux --layout-preview
```

## Build from source

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

Pushing a tag like `v0.1.6` triggers `.github/workflows/release.yml`, which builds macOS arm64 and x86_64 archives and publishes a GitHub prerelease.
