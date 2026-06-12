#!/usr/bin/env bash
#
# tuimux installer — macOS / Linux.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/hungryZoo/tuimux/main/scripts/install.sh | bash
#
# Environment variables:
#   TUIMUX_VERSION       Tag to install (e.g. v0.2.0-alpha.3). Default: latest prerelease/release.
#   TUIMUX_INSTALL_DIR   Where to put the binary. Default: ~/.local/bin, falling
#                        back to /usr/local/bin if the former isn't writable.
#   TUIMUX_TMUX_CONF     tmux config file to update. Default: ~/.tmux.conf.
#
set -euo pipefail

REPO="hungryZoo/tuimux"
BINARY="tuimux"

# ---------------------------------------------------------------------------
# Pretty output helpers
# ---------------------------------------------------------------------------
if [ -t 1 ]; then
  BOLD=$(printf '\033[1m'); RED=$(printf '\033[31m'); GREEN=$(printf '\033[32m')
  YELLOW=$(printf '\033[33m'); RESET=$(printf '\033[0m')
else
  BOLD=""; RED=""; GREEN=""; YELLOW=""; RESET=""
fi
info()  { printf '%s==>%s %s\n' "$GREEN" "$RESET" "$*"; }
warn()  { printf '%swarn:%s %s\n' "$YELLOW" "$RESET" "$*" >&2; }
err()   { printf '%serror:%s %s\n' "$RED" "$RESET" "$*" >&2; }
die()   { err "$*"; exit 1; }

# ---------------------------------------------------------------------------
# Preconditions
# ---------------------------------------------------------------------------
need_cmd() { command -v "$1" >/dev/null 2>&1 || die "required command not found: $1"; }
need_cmd uname
need_cmd curl
need_cmd tar
need_cmd mktemp
need_cmd dirname

OS="$(uname -s)"
RAW_ARCH="$(uname -m)"

# Detect OS/architecture and map to the Rust target triple used in asset names.
case "$OS:$RAW_ARCH" in
  Darwin:arm64|Darwin:aarch64) TARGET="aarch64-apple-darwin" ;;
  Darwin:x86_64|Darwin:amd64) TARGET="x86_64-apple-darwin" ;;
  Linux:x86_64|Linux:amd64) TARGET="x86_64-unknown-linux-gnu" ;;
  Linux:aarch64|Linux:arm64) TARGET="aarch64-unknown-linux-gnu" ;;
  Linux:armv7l|Linux:armv7) TARGET="armv7-unknown-linux-gnueabihf" ;;
  Darwin:*) die "unsupported macOS architecture: $RAW_ARCH" ;;
  Linux:armv6l) die "unsupported Raspberry Pi armv6 architecture: $RAW_ARCH; use a 32-bit armv7 or 64-bit arm64 OS image." ;;
  Linux:*) die "unsupported Linux architecture: $RAW_ARCH" ;;
  *) die "this installer currently supports macOS and Linux only; detected: $OS / $RAW_ARCH" ;;
esac
info "Detected ${OS} / ${RAW_ARCH} (target: ${TARGET})"

# tmux is a hard runtime dependency (tuimux is a tmux front-end).
if ! command -v tmux >/dev/null 2>&1; then
  warn "tmux is not installed — tuimux needs it at runtime."
  case "$OS" in
    Darwin) warn "Install it with:  ${BOLD}brew install tmux${RESET}" ;;
    Linux) warn "Install it with your package manager, e.g. ${BOLD}sudo apt install tmux${RESET} or ${BOLD}sudo dnf install tmux${RESET}" ;;
  esac
else
  info "Found tmux: $(tmux -V 2>/dev/null || echo 'unknown version')"
fi

tmux_conf_has_option() {
  # $1 = tmux option name, e.g. mouse or history-limit
  # Match active `set` / `set-option` lines and ignore comments/blank lines.
  local option="$1"
  [ -f "$TMUX_CONF" ] || return 1
  awk -v option="$option" '
    /^[[:space:]]*($|#)/ { next }
    {
      for (i = 1; i <= NF; i++) {
        if ($i == option) {
          for (j = 1; j < i; j++) {
            if ($j == "set" || $j == "set-option") {
              found = 1
            }
          }
        }
      }
    }
    END { exit found ? 0 : 1 }
  ' "$TMUX_CONF"
}

ensure_tmux_conf_option() {
  # $1 = option name, $2 = value
  local option="$1"
  local value="$2"
  if tmux_conf_has_option "$option"; then
    info "tmux ${option} already configured in ${TMUX_CONF}"
    return
  fi
  mkdir -p "$(dirname "$TMUX_CONF")"
  if [ ! -f "$TMUX_CONF" ]; then
    : > "$TMUX_CONF"
  fi
  {
    printf '\n# Added by tuimux installer\n'
    printf 'set -g %s %s\n' "$option" "$value"
  } >> "$TMUX_CONF"
  info "Added tmux ${option} ${value} to ${TMUX_CONF}"
}

TMUX_CONF="${TUIMUX_TMUX_CONF:-${HOME}/.tmux.conf}"
ensure_tmux_conf_option mouse on
ensure_tmux_conf_option history-limit 100000

# ---------------------------------------------------------------------------
# Resolve version
# ---------------------------------------------------------------------------
API="https://api.github.com/repos/${REPO}"

api_get() {
  # $1 = API path suffix (e.g. /releases?per_page=20)
  curl -fsSL     -H "Accept: application/vnd.github+json"     "${API}$1"
}

VERSION="${TUIMUX_VERSION:-}"
if [ -z "$VERSION" ]; then
  info "Resolving latest release/prerelease tag…"
  # GitHub's /releases/latest intentionally ignores prereleases. tuimux is still
  # prerelease-only, so list releases and pick the newest tag instead.
  RELEASES_JSON="$(api_get '/releases?per_page=20')"
  # Do not use `grep -m1` in a pipe under `set -o pipefail`: grep may close the
  # pipe early, make the writer receive SIGPIPE, and terminate the installer
  # right after the "Resolving…" line. Let sed consume the whole JSON instead.
  VERSION="$(printf '%s\n' "$RELEASES_JSON" | sed -n -E 's/.*"tag_name" *: *"([^"]+)".*/\1/p' | sed -n '1p')"
  [ -n "$VERSION" ] || die "could not determine the latest release tag. Set TUIMUX_VERSION=vX.Y.Z explicitly."
fi
info "Installing ${BINARY} ${BOLD}${VERSION}${RESET}"

ASSET="${BINARY}-${VERSION}-${TARGET}.tar.gz"
DOWNLOAD_URL="https://github.com/${REPO}/releases/download/${VERSION}/${ASSET}"

# ---------------------------------------------------------------------------
# Download, verify checksum (if available), extract
# ---------------------------------------------------------------------------
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

info "Downloading ${ASSET}…"
curl -fSL -o "${TMP}/${ASSET}" "$DOWNLOAD_URL"   || die "download failed from ${DOWNLOAD_URL}"

# Best-effort SHA256 verification against the published SHA256SUMS, if present.
sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{print $1}'
  else
    return 1
  fi
}

SUMS_URL="https://github.com/${REPO}/releases/download/${VERSION}/SHA256SUMS"
if curl -fsSL -o "${TMP}/SHA256SUMS" "$SUMS_URL" 2>/dev/null; then
  EXPECTED="$(grep " ${ASSET}\$" "${TMP}/SHA256SUMS" | awk '{print $1}')"
  if [ -n "$EXPECTED" ]; then
    ACTUAL="$(sha256_file "${TMP}/${ASSET}" || true)"
    [ -n "$ACTUAL" ] || die "could not verify checksum: install sha256sum or shasum"
    if [ "$EXPECTED" = "$ACTUAL" ]; then
      info "Checksum OK (${ACTUAL:0:12}…)"
    else
      die "checksum mismatch! expected ${EXPECTED}, got ${ACTUAL}"
    fi
  fi
else
  warn "no SHA256SUMS published for ${VERSION}; skipping checksum verification."
fi

info "Extracting…"
tar -xzf "${TMP}/${ASSET}" -C "$TMP"
BIN_PATH="$(find "$TMP" -type f -name "$BINARY" -perm -u+x | head -n1)"
[ -n "$BIN_PATH" ] || die "could not find '${BINARY}' inside the archive."

# ---------------------------------------------------------------------------
# Choose install dir and install
# ---------------------------------------------------------------------------
choose_dir() {
  if [ -n "${TUIMUX_INSTALL_DIR:-}" ]; then echo "$TUIMUX_INSTALL_DIR"; return; fi
  local existing=""
  existing="$(command -v "$BINARY" 2>/dev/null || true)"
  if [ -n "$existing" ]; then dirname "$existing"; return; fi
  local home_bin="${HOME}/.local/bin"
  mkdir -p "$home_bin" 2>/dev/null || true
  if [ -w "$home_bin" ] || [ ! -e "$home_bin" ]; then echo "$home_bin"; return; fi
  echo "/usr/local/bin"
}
INSTALL_DIR="$(choose_dir)"
mkdir -p "$INSTALL_DIR" 2>/dev/null || true

DEST="${INSTALL_DIR}/${BINARY}"
if [ -w "$INSTALL_DIR" ] || mkdir -p "$INSTALL_DIR" 2>/dev/null; then
  install -m 0755 "$BIN_PATH" "$DEST" 2>/dev/null || cp "$BIN_PATH" "$DEST"
elif command -v sudo >/dev/null 2>&1; then
  warn "${INSTALL_DIR} is not writable; using sudo."
  sudo install -m 0755 "$BIN_PATH" "$DEST"
else
  die "${INSTALL_DIR} is not writable and sudo is unavailable. Set TUIMUX_INSTALL_DIR to a writable path."
fi
chmod 0755 "$DEST" 2>/dev/null || true

# macOS Gatekeeper: strip the quarantine attribute so the binary runs without a
# scary prompt (assets from CI are not notarized in this MVP).
if [ "$OS" = "Darwin" ]; then
  xattr -d com.apple.quarantine "$DEST" >/dev/null 2>&1 || true
fi

info "Installed ${BOLD}${BINARY}${RESET} to ${BOLD}${DEST}${RESET}"

# PATH hint.
case ":${PATH}:" in
  *":${INSTALL_DIR}:"*) : ;;
  *) warn "${INSTALL_DIR} is not on your PATH. Add it, e.g.:
       echo 'export PATH=\"${INSTALL_DIR}:\$PATH\"' >> ~/.zshrc && source ~/.zshrc" ;;
esac

info "Done. Verify with:  ${BOLD}${BINARY} --version${RESET}  and  ${BOLD}${BINARY} --doctor${RESET}"
