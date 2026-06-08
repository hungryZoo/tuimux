# tuimux

`tuimux` is an early Rust MVP for a VS Code-inspired, mouse-first TUI front-end for tmux.

The long-term goal is to use `tmux` as the backend/session engine and provide a simpler full-TUI wrapper:

- left file explorer with file sizes
- center tmux pane area
- right sidebar with current session and vertical window tabs
- bottom clickable menu bar with **Detach**
- no tmux prefix-key workflow for normal operations

See:

- [PRD](docs/prd.md)
- [SRS](docs/srs.md)

## Current prerelease scope

This is not the complete tmux control-mode implementation yet. The current binary is a testable MVP scaffold that includes:

- `tuimux --help`
- `tuimux --version`
- `tuimux --doctor` to check tmux and terminal readiness
- `tuimux --layout-preview` to render the planned VS Code-like layout
- a safe interactive TUI shell that shows the planned layout and exits with `q`/`Esc`

## macOS install

Runtime dependency:

```sh
brew install tmux
```

### Public repo one-liner

```sh
curl -fsSL https://raw.githubusercontent.com/hungryZoo/tuimux/main/scripts/install.sh | bash
```

### Private repo one-liner

Because this repository is currently private, both the installer script and release assets need GitHub authentication. Use a token with `repo` scope:

```sh
export GITHUB_TOKEN="<github-token-with-repo-scope>"
curl -H "Authorization: Bearer $GITHUB_TOKEN" \
  -fsSL https://raw.githubusercontent.com/hungryZoo/tuimux/main/scripts/install.sh \
  | bash
```

Install a specific prerelease tag:

```sh
TUIMUX_VERSION=v0.1.0 \
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

Pushing a tag like `v0.1.0` triggers `.github/workflows/release.yml`, which builds macOS arm64 and x86_64 archives and publishes a GitHub prerelease.
