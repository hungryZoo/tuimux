# tuimux

`tuimux` is an early Rust MVP for a prefix-free, mouse-first TUI wrapper around tmux.

v0.1.9 restores the expected tuimux TUI as the default. Running `tuimux` opens the ratatui interface with a live tmux-backed main pane plus right sidebar session/window controls. v0.1.7 accidentally made the default command launch plain tmux with no tuimux UI; that path is now opt-in only via the hidden `--native-client` fallback.

See:

- [PRD](docs/prd.md)
- [SRS](docs/srs.md)

## Current prerelease scope

This is still a 0.x prerelease. Current behavior:

- `tuimux` opens the tuimux TUI by default.
- The sidebar shows the current session, Detach, windows, `✕` window close buttons, and `+ new`.
- The session dialog opens by default and includes session selection, `New Session`, and `Detach`.
- Session/window operations use real tmux commands.
- The main pane captures tmux with SGR style escapes, renders ANSI color/style spans, preserves blank rows to avoid stale glyphs after `clear`, and resizes the tmux window to the TUI main-area size when the viewport changes.
- `tuimux --native-client` is a hidden fallback that opens a plain tmux client if needed.
- `tuimux --doctor`, `--version`, and `--layout-preview` remain available.

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
TUIMUX_VERSION=v0.1.9 \
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

Pushing a tag like `v0.1.9` triggers `.github/workflows/release.yml`, which builds macOS arm64 and x86_64 archives and publishes a GitHub prerelease.
