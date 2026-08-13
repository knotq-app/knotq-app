#!/bin/sh
# KnotQ Linux installer
# Usage: curl -fsSL https://knotq.com/install.sh | sh
set -e

REPO="knotq-app/knotq-app"
BIN_DIR="${KNOTQ_INSTALL_DIR:-$HOME/.local/bin}"
APP_ROOT="${KNOTQ_INSTALL_ROOT:-${XDG_DATA_HOME:-$HOME/.local/share}/knotq/app}"

echo "Installing KnotQ..."

# Detect architecture
ARCH=$(uname -m)
case "$ARCH" in
  x86_64|amd64) ARCH_SUFFIX="linux-x86_64" ;;
  aarch64|arm64)
    echo "KnotQ does not currently publish a Linux ARM64 build."
    exit 1
    ;;
  *) echo "Unsupported architecture: $ARCH"; exit 1 ;;
esac

# Get latest release URL
RELEASE_URL="https://api.github.com/repos/$REPO/releases/latest"
ASSET_URL=$(curl -fsSL "$RELEASE_URL" | grep -o "https://[^\"]*KnotQ-[^\"]*${ARCH_SUFFIX}\.tar\.gz" | head -1)

if [ -z "$ASSET_URL" ]; then
  echo "Could not find a release for $ARCH_SUFFIX"
  echo "Check https://github.com/$REPO/releases for available downloads."
  exit 1
fi

# Download and extract
TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT HUP INT TERM
echo "Downloading $ASSET_URL..."
curl -fsSL "$ASSET_URL" -o "$TMPDIR/knotq.tar.gz"

mkdir -p "$TMPDIR/extract" "$APP_ROOT" "$BIN_DIR"
tar xzf "$TMPDIR/knotq.tar.gz" -C "$TMPDIR/extract"
test -f "$TMPDIR/extract/knotq"
test -d "$TMPDIR/extract/assets"
install -m 0755 "$TMPDIR/extract/knotq" "$APP_ROOT/knotq"
# Assets live in KnotQ's private application root. This prevents an update from
# pruning an unrelated, generically named ~/.local/bin/assets directory.
rm -rf "$APP_ROOT/assets"
cp -R "$TMPDIR/extract/assets" "$APP_ROOT/assets"
rm -f "$BIN_DIR/knotq"
ln -s "$APP_ROOT/knotq" "$BIN_DIR/knotq"

echo ""
echo "KnotQ installed to $APP_ROOT (launcher: $BIN_DIR/knotq)"

# Check if install dir is in PATH
case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *)
    echo ""
    echo "Add $BIN_DIR to your PATH:"
    echo "  export PATH=\"$BIN_DIR:\$PATH\""
    echo ""
    echo "Or add that line to your ~/.bashrc or ~/.zshrc"
    ;;
esac

echo "Run 'knotq' to start."
