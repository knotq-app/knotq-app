#!/bin/sh
set -eu

installer="${1:-bundling/install.sh}"

grep -q 'does not currently publish a Linux ARM64 build' "$installer"
grep -q 'APP_ROOT=.*XDG_DATA_HOME' "$installer"
grep -q 'rm -rf "$APP_ROOT/assets"' "$installer"
grep -q 'test -d "$TMPDIR/extract/assets"' "$installer"
grep -q 'rm -f "$BIN_DIR/knotq"' "$installer"
grep -q 'ln -s "$APP_ROOT/knotq" "$BIN_DIR/knotq"' "$installer"

if grep -q 'tar xzf .* -C "$INSTALL_DIR"' "$installer"; then
  echo "installer still extracts its generic assets directory into the binary directory" >&2
  exit 1
fi

# Regression: the pre-isolation installer left a regular executable at this
# exact launcher path. The migration must replace it with the private-root link.
temporary="$(mktemp -d)"
trap 'rm -rf "$temporary"' EXIT HUP INT TERM
mkdir -p "$temporary/bin" "$temporary/app"
touch "$temporary/bin/knotq" "$temporary/app/knotq"
rm -f "$temporary/bin/knotq"
ln -s "$temporary/app/knotq" "$temporary/bin/knotq"
test -L "$temporary/bin/knotq"
test "$(readlink "$temporary/bin/knotq")" = "$temporary/app/knotq"
