#!/usr/bin/env bash
#
# tuimux installer — macOS.
#
# Usage (public repo):
#   curl -fsSL https://raw.githubusercontent.com/hungryZoo/tuimux/main/scripts/install.sh | bash
#
# Usage (private repo — needs a token with `repo` scope to read raw files and releases):
#   export GITHUB_TOKEN="TOKEN"
#   curl -H "Authorization: Bearer $GITHUB_TOKEN" \
#     -fsSL https://raw.githubusercontent.com/hungryZoo/tuimux/main/scripts/install.sh \
#     | bash
#
# Environment variables:
#   TUIMUX_VERSION       Tag to install (e.g. v0.1.0). Default: latest release.
#   TUIMUX_INSTALL_DIR   Where to put the binary. Default: ~/.local/bin, falling
#                        back to /usr/local/bin if the former isn't writable.
#   GITHUB_TOKEN         If set, releases are fetched via the GitHub API (required
#                        for private repos). Without it, public download URLs are
#                        used directly so the curl|bash one-liner needs no auth.
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

OS="$(uname -s)"
if [ "$OS" != "Darwin" ]; then
  die "this installer currently supports macOS (Darwin) only; detected: $OS.
     For Linux, build from source with: cargo install --path ."
fi

# Detect architecture and map to the Rust target triple used in asset names.
RAW_ARCH="$(uname -m)"
case "$RAW_ARCH" in
  arm64|aarch64) ARCH="aarch64" ;;
  x86_64|amd64)  ARCH="x86_64"  ;;
  *) die "unsupported macOS architecture: $RAW_ARCH" ;;
esac
TARGET="${ARCH}-apple-darwin"
info "Detected macOS / ${RAW_ARCH} (target: ${TARGET})"

# tmux is a hard runtime dependency (tuimux is a tmux front-end).
if ! command -v tmux >/dev/null 2>&1; then
  warn "tmux is not installed — tuimux needs it at runtime."
  warn "Install it with:  ${BOLD}brew install tmux${RESET}"
else
  info "Found tmux: $(tmux -V 2>/dev/null || echo 'unknown version')"
fi

# ---------------------------------------------------------------------------
# Resolve version
# ---------------------------------------------------------------------------
API="https://api.github.com/repos/${REPO}"
AUTH_ARGS=()
if [ -n "${GITHUB_TOKEN:-}" ]; then
  AUTH_ARGS=(-H "Authorization: Bearer ${GITHUB_TOKEN}")
  info "Using GITHUB_TOKEN for authenticated GitHub API access (private repo OK)."
fi

api_get() {
  # $1 = API path suffix (e.g. /releases/latest)
  curl -fsSL "${AUTH_ARGS[@]}" -H "Accept: application/vnd.github+json" "${API}$1"
}

VERSION="${TUIMUX_VERSION:-}"
if [ -z "$VERSION" ]; then
  info "Resolving latest release tag…"
  if [ -n "${GITHUB_TOKEN:-}" ]; then
    # Private repos must go through the API.
    VERSION="$(api_get /releases/latest | grep -m1 '"tag_name"' | sed -E 's/.*"tag_name" *: *"([^"]+)".*/\1/')"
  else
    # Public: follow the /releases/latest redirect to read the tag without auth.
    VERSION="$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/${REPO}/releases/latest" | sed -E 's#.*/tag/##')"
  fi
  [ -n "$VERSION" ] && [ "$VERSION" != "releases" ] || die "could not determine the latest release tag. Set TUIMUX_VERSION=vX.Y.Z explicitly."
fi
info "Installing ${BINARY} ${BOLD}${VERSION}${RESET}"

ASSET="${BINARY}-${VERSION}-${TARGET}.tar.gz"

# ---------------------------------------------------------------------------
# Determine the asset download URL
# ---------------------------------------------------------------------------
if [ -n "${GITHUB_TOKEN:-}" ]; then
  # Private repos: resolve the asset's API id, then download via the API with the
  # octet-stream Accept header (this is what makes private-asset download work).
  info "Looking up asset id via GitHub API…"
  ASSET_ID="$(api_get "/releases/tags/${VERSION}" \
    | tr ',' '\n' \
    | grep -B2 "\"name\": \"${ASSET}\"" \
    | grep '"id"' | head -n1 | grep -oE '[0-9]+')"
  [ -n "$ASSET_ID" ] || die "asset ${ASSET} not found in release ${VERSION}."
  DOWNLOAD_URL="${API}/releases/assets/${ASSET_ID}"
  DL_ARGS=("${AUTH_ARGS[@]}" -H "Accept: application/octet-stream")
else
  DOWNLOAD_URL="https://github.com/${REPO}/releases/download/${VERSION}/${ASSET}"
  DL_ARGS=()
fi

# ---------------------------------------------------------------------------
# Download, verify checksum (if available), extract
# ---------------------------------------------------------------------------
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

info "Downloading ${ASSET}…"
curl -fSL "${DL_ARGS[@]}" -o "${TMP}/${ASSET}" "$DOWNLOAD_URL" \
  || die "download failed from ${DOWNLOAD_URL}"

# Best-effort SHA256 verification against the published SHA256SUMS, if present.
if [ -z "${GITHUB_TOKEN:-}" ]; then
  SUMS_URL="https://github.com/${REPO}/releases/download/${VERSION}/SHA256SUMS"
  if curl -fsSL -o "${TMP}/SHA256SUMS" "$SUMS_URL" 2>/dev/null; then
    EXPECTED="$(grep " ${ASSET}\$" "${TMP}/SHA256SUMS" | awk '{print $1}')"
    if [ -n "$EXPECTED" ]; then
      ACTUAL="$(shasum -a 256 "${TMP}/${ASSET}" | awk '{print $1}')"
      if [ "$EXPECTED" = "$ACTUAL" ]; then
        info "Checksum OK (${ACTUAL:0:12}…)"
      else
        die "checksum mismatch! expected ${EXPECTED}, got ${ACTUAL}"
      fi
    fi
  else
    warn "no SHA256SUMS published for ${VERSION}; skipping checksum verification."
  fi
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
xattr -d com.apple.quarantine "$DEST" >/dev/null 2>&1 || true

info "Installed ${BOLD}${BINARY}${RESET} to ${BOLD}${DEST}${RESET}"

# PATH hint.
case ":${PATH}:" in
  *":${INSTALL_DIR}:"*) : ;;
  *) warn "${INSTALL_DIR} is not on your PATH. Add it, e.g.:
       echo 'export PATH=\"${INSTALL_DIR}:\$PATH\"' >> ~/.zshrc && source ~/.zshrc" ;;
esac

info "Done. Verify with:  ${BOLD}${BINARY} --version${RESET}  and  ${BOLD}${BINARY} --doctor${RESET}"
