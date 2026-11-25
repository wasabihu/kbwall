#!/bin/sh
# One-line installer for kbwall prebuilt release binaries (macOS & Linux).
# Verifies SHA-256 checksums before installation.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/wasabihu/kbwall/main/install.sh | sh
#
# Optional environment variables:
#   KBWALL_VERSION=v0.1.0        Pin a specific release tag (default: latest)
#   KBWALL_INSTALL_DIR=~/.local/bin

set -eu

REPO="wasabihu/kbwall"
INSTALL_DIR="${KBWALL_INSTALL_DIR:-$HOME/.local/bin}"

need_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    printf 'error: required command not found: %s\n' "$1" >&2
    exit 1
  fi
}

need_cmd curl
need_cmd tar
need_cmd mktemp

OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS" in
  Darwin)
    case "$ARCH" in
      arm64|aarch64) TARGET="aarch64-apple-darwin" ;;
      x86_64|amd64)  TARGET="x86_64-apple-darwin" ;;
      *)
        printf 'error: unsupported macOS architecture: %s\n' "$ARCH" >&2
        exit 1
        ;;
    esac
    ;;
  Linux)
    case "$ARCH" in
      x86_64|amd64)  TARGET="x86_64-unknown-linux-gnu" ;;
      aarch64|arm64) TARGET="aarch64-unknown-linux-gnu" ;;
      *)
        printf 'error: unsupported Linux architecture: %s\n' "$ARCH" >&2
        exit 1
        ;;
    esac
    ;;
  *)
    printf 'error: unsupported OS: %s (download Windows .zip from GitHub Releases)\n' "$OS" >&2
    exit 1
    ;;
esac

if [ -n "${KBWALL_VERSION:-}" ]; then
  VERSION="$KBWALL_VERSION"
else
  FINAL_URL="$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/${REPO}/releases/latest")"
  VERSION="${FINAL_URL##*/}"
  if [ -z "$VERSION" ] || [ "$VERSION" = "latest" ] || [ "$VERSION" = "releases" ]; then
    printf 'error: failed to resolve latest release tag for %s\n' "$REPO" >&2
    exit 1
  fi
fi

ARCHIVE="kbwall-${TARGET}.tar.gz"
BASE_URL="https://github.com/${REPO}/releases/download/${VERSION}"

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT INT TERM

printf 'Downloading kbwall %s (%s)...\n' "$VERSION" "$TARGET"
curl -fsSL "${BASE_URL}/${ARCHIVE}" -o "${TMP_DIR}/${ARCHIVE}"
curl -fsSL "${BASE_URL}/SHA256SUMS.txt" -o "${TMP_DIR}/SHA256SUMS.txt"

EXPECTED="$(awk -v f="$ARCHIVE" '$2 == f || $2 == ("*" f) { print $1; exit }' "${TMP_DIR}/SHA256SUMS.txt")"
if [ -z "$EXPECTED" ]; then
  printf 'error: checksum entry for %s not found in SHA256SUMS.txt\n' "$ARCHIVE" >&2
  exit 1
fi

if command -v sha256sum >/dev/null 2>&1; then
  ACTUAL="$(sha256sum "${TMP_DIR}/${ARCHIVE}" | awk '{ print $1 }')"
elif command -v shasum >/dev/null 2>&1; then
  ACTUAL="$(shasum -a 256 "${TMP_DIR}/${ARCHIVE}" | awk '{ print $1 }')"
else
  printf 'error: neither sha256sum nor shasum found to verify archive integrity\n' >&2
  exit 1
fi

if [ "$ACTUAL" != "$EXPECTED" ]; then
  printf 'error: SHA-256 mismatch for %s\n  expected: %s\n  actual:   %s\n' "$ARCHIVE" "$EXPECTED" "$ACTUAL" >&2
  exit 1
fi

tar -xzf "${TMP_DIR}/${ARCHIVE}" -C "${TMP_DIR}"
mkdir -p "$INSTALL_DIR"
TMP_BIN="${INSTALL_DIR}/.kbwall.tmp.$$"
cp "${TMP_DIR}/kbwall" "$TMP_BIN"
chmod 755 "$TMP_BIN"
mv "$TMP_BIN" "${INSTALL_DIR}/kbwall"

printf 'Installed %s -> %s/kbwall\n' "$("${INSTALL_DIR}/kbwall" --version)" "$INSTALL_DIR"

case ":${PATH}:" in
  *":${INSTALL_DIR}:"*) ;;
  *)
    printf 'Note: add %s to your PATH, e.g.:\n  export PATH="%s:$PATH"\n' "$INSTALL_DIR" "$INSTALL_DIR"
    ;;
esac
