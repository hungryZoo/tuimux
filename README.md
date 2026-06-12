# tuimux

`tuimux` is an early Rust MVP for a prefix-free, mouse-first TUI wrapper around tmux.

v0.2.0-alpha.1 keeps the tuimux ratatui UI as the default and replaces the old snapshot-style main pane with a PTY-backed tmux terminal surface. Running `tuimux` opens the tuimux UI with a real tmux-backed main pane plus right sidebar session/window controls. Plain tmux remains opt-in only via the hidden `--native-client` fallback.

See:

- [SRS](docs/srs.md)
- [SDD](docs/sdd.md)
- [PRD](docs/prd.md)

## Current prerelease scope

This is still a 0.x prerelease. Current behavior:

- `tuimux` opens the tuimux TUI by default.
- The sidebar shows the current session, Detach, windows, `✕` window close buttons, and `+ new`.
- The session dialog opens by default and includes session selection, `New Session`, and `Detach`.
- Session/window operations use real tmux commands.
- The main pane runs a real tmux client inside a PTY, parses the byte stream with `vt100`, renders terminal cells with ratatui spans, and resizes the PTY to the TUI main-area size.
- `tuimux --native-client` is a hidden fallback that opens a plain tmux client if needed.
- `tuimux --doctor`, `--version`, and `--layout-preview` remain available.

## macOS Apple Silicon install

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
TUIMUX_VERSION=v0.2.0-alpha.1 \
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
tuimux
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

Pushing a tag like `v0.2.0-alpha.1` triggers `.github/workflows/release.yml`, which builds a macOS Apple Silicon archive and publishes a GitHub prerelease.
