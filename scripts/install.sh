#!/usr/bin/env bash
#
# tuimux installer — macOS arm64 prerelease.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/hungryZoo/tuimux/v0.2.0-alpha.13/scripts/install.sh | \
#     TUIMUX_VERSION=v0.2.0-alpha.13 bash
#
# Environment variables:
#   TUIMUX_VERSION       Tag to install (e.g. v0.2.0-alpha.13). Default: latest prerelease/release.
#   TUIMUX_INSTALL_DIR   Where to put the binary. Default: ~/.local/bin, falling
#                        back to /usr/local/bin if the former isn't writable.
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

# This prerelease publishes macOS Apple Silicon assets first. Other targets will
# be re-enabled once the Rust-native multiplexer has been tested there.
case "$OS:$RAW_ARCH" in
  Darwin:arm64|Darwin:aarch64) TARGET="aarch64-apple-darwin" ;;
  Darwin:*) die "this prerelease currently supports macOS Apple Silicon only; detected macOS ${RAW_ARCH}" ;;
  *) die "this prerelease currently supports macOS Apple Silicon only; detected: ${OS} / ${RAW_ARCH}" ;;
esac
info "Detected ${OS} / ${RAW_ARCH} (target: ${TARGET})"

info "tmux is optional. The default tuimux UI now uses the Rust-native multiplexer."

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
