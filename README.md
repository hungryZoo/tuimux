# tuimux

`tuimux` is an early Rust MVP for a prefix-free, mouse-first TUI wrapper around tmux.

v0.2.0-alpha.4 keeps the tuimux ratatui UI as the default and replaces the old snapshot-style main pane with a PTY-backed tmux terminal surface. Running `tuimux` opens the tuimux UI with a real tmux-backed main pane plus right sidebar session/window controls. Plain tmux remains opt-in only via the hidden `--native-client` fallback.

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
- Mouse events in terminal input mode are passed through to tmux's mouse protocol, including Shift-modified mouse events.
- `tuimux --native-client` is a hidden fallback that opens a plain tmux client if needed.
- `tuimux --doctor`, `--version`, and `--layout-preview` remain available.

## Install

Runtime dependency: `tmux`.

```sh
# macOS
brew install tmux

# Debian/Ubuntu
sudo apt install tmux

# Fedora/RHEL
sudo dnf install tmux
```

### macOS / Linux one-line installer

```sh
curl -fsSL https://raw.githubusercontent.com/hungryZoo/tuimux/main/scripts/install.sh | bash
```

Install a specific prerelease tag:

```sh
curl -fsSL https://raw.githubusercontent.com/hungryZoo/tuimux/main/scripts/install.sh | \
  TUIMUX_VERSION=v0.2.0-alpha.4 bash
```

Custom install directory:

```sh
curl -fsSL https://raw.githubusercontent.com/hungryZoo/tuimux/main/scripts/install.sh | \
  TUIMUX_INSTALL_DIR="$HOME/bin" bash
```

The shell installer also ensures these tmux defaults exist in `~/.tmux.conf` without overwriting existing settings:

```tmux
set -g mouse on
set -g history-limit 100000
```

### Windows PowerShell installer

```powershell
irm https://raw.githubusercontent.com/hungryZoo/tuimux/main/scripts/install.ps1 | iex
```

Windows builds are published as zip archives. `tmux` still needs to be available on `PATH`, typically through MSYS2, Cygwin, or WSL interop.

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

Pushing a tag like `v0.2.0-alpha.4` triggers `.github/workflows/release.yml`, which publishes a GitHub prerelease with macOS, Windows, Linux tarballs, Linux `.deb`/`.rpm` packages, and Raspberry Pi arm64/armv7 assets.
