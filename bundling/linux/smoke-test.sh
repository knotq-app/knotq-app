#!/usr/bin/env bash
set -euo pipefail

# smoke-test.sh <tarball>
# Installs KnotQ from the release tarball and fails unless the app starts.
# Needs a display (xvfb-run) and a Vulkan driver (lavapipe on CI).

TARBALL="${1:?usage: $0 <tarball>}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
source "$ROOT/bundling/smoke-test-common.sh"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/install"
tar -xzf "$TARBALL" -C "$work/install"
test -x "$work/install/knotq-mcp" || { echo "[error] knotq-mcp missing from tarball" >&2; exit 1; }

smoke_launch "$work/install/knotq" "$work/run"
echo "[ok] Linux smoke test passed"
