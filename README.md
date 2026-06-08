# tuimux

`tuimux` is an early Rust MVP for a prefix-free, mouse-first wrapper around tmux.

The v0.1.7 reset is intentionally **tmux-native**: the default command opens a real tmux client instead of scraping `capture-pane` and replaying input with `send-keys`. That means `ls`, `clear`, `nano`/`vim`/`less`, mouse wheel/copy-mode, and Korean/CJK text are handled by tmux and your terminal — not by a fake shell renderer.

See:

- [PRD](docs/prd.md)
- [SRS](docs/srs.md)

## Current prerelease scope

This is still a 0.x prerelease. Current behavior:

- `tuimux` creates/attaches session `tuimux` through real tmux.
- `tuimux --session dev` creates/attaches/switches to session `dev`.
- tmux mouse mode is enabled with `tmux set-option -gq mouse on`.
- outside tmux: runs `tmux -u attach-session -t <session>`.
- inside tmux: runs `tmux switch-client -t <session>` to avoid nested clients.
- `tuimux --doctor`, `--version`, and `--layout-preview` remain available.
- the old ratatui dashboard prototype is hidden behind `--dashboard` and is not the default shell experience.

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
TUIMUX_VERSION=v0.1.7 \
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
tuimux --session dev
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

Pushing a tag like `v0.1.7` triggers `.github/workflows/release.yml`, which builds macOS arm64 and x86_64 archives and publishes a GitHub prerelease.
