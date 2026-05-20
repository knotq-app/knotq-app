#!/bin/sh
# KnotQ Linux installer
# Usage: curl -fsSL https://knotq.com/install.sh | sh
set -e

REPO="knotq-app/knotq-app"
INSTALL_DIR="${KNOTQ_INSTALL_DIR:-$HOME/.local/bin}"

echo "Installing KnotQ..."

# Detect architecture
ARCH=$(uname -m)
case "$ARCH" in
  x86_64|amd64) ARCH_SUFFIX="linux-x86_64" ;;
  aarch64|arm64) ARCH_SUFFIX="linux-aarch64" ;;
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
echo "Downloading $ASSET_URL..."
curl -fsSL "$ASSET_URL" -o "$TMPDIR/knotq.tar.gz"

mkdir -p "$INSTALL_DIR"
tar xzf "$TMPDIR/knotq.tar.gz" -C "$INSTALL_DIR"
chmod +x "$INSTALL_DIR/knotq"
rm -rf "$TMPDIR"

echo ""
echo "KnotQ installed to $INSTALL_DIR/knotq"

# Check if install dir is in PATH
case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *)
    echo ""
    echo "Add $INSTALL_DIR to your PATH:"
    echo "  export PATH=\"$INSTALL_DIR:\$PATH\""
    echo ""
    echo "Or add that line to your ~/.bashrc or ~/.zshrc"
    ;;
esac

echo "Run 'knotq' to start."
