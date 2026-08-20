#!/usr/bin/env sh
set -e

REPO="nkitsaini/sanemark"
BINARY="sanemark"
INSTALL_DIR="${INSTALL_DIR:-$HOME/.local/bin}"

OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS" in
  Linux)
    case "$ARCH" in
      x86_64) TARGET="x86_64-unknown-linux-musl" ;;
      aarch64|arm64) TARGET="aarch64-unknown-linux-musl" ;;
      *) echo "Error: Unsupported Linux architecture: $ARCH" >&2; exit 1 ;;
    esac
    ;;
  Darwin)
    case "$ARCH" in
      x86_64) TARGET="x86_64-apple-darwin" ;;
      arm64|aarch64) TARGET="aarch64-apple-darwin" ;;
      *) echo "Error: Unsupported macOS architecture: $ARCH" >&2; exit 1 ;;
    esac
    ;;
  *)
    echo "Error: Unsupported operating system: $OS" >&2
    exit 1
    ;;
esac

LATEST_TAG="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" | grep '"tag_name":' | sed -E 's/.*"([^"]+)".*/\1/')"
if [ -z "$LATEST_TAG" ]; then
  echo "Error: Could not determine latest release tag." >&2
  exit 1
fi

URL="https://github.com/${REPO}/releases/download/${LATEST_TAG}/sanemark-${LATEST_TAG}-${TARGET}.tar.gz"

echo "Downloading sanemark ${LATEST_TAG} for ${TARGET}..."
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

curl -fsSL "$URL" | tar -xz -C "$TMPDIR"

mkdir -p "$INSTALL_DIR"
cp "$TMPDIR/sanemark-${LATEST_TAG}-${TARGET}/sanemark" "$INSTALL_DIR/$BINARY"
chmod +x "$INSTALL_DIR/$BINARY"

if [ -f "$TMPDIR/sanemark-${LATEST_TAG}-${TARGET}/sanemark-lsp" ]; then
  cp "$TMPDIR/sanemark-${LATEST_TAG}-${TARGET}/sanemark-lsp" "$INSTALL_DIR/sanemark-lsp"
  chmod +x "$INSTALL_DIR/sanemark-lsp"
fi

echo "Successfully installed sanemark to $INSTALL_DIR/$BINARY"
echo "Make sure $INSTALL_DIR is in your PATH."
