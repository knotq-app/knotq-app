#!/usr/bin/env bash
#
# One chokepoint for destroying a scheme.
#
# `Workspace::remove_scheme_completely` clears every reference a scheme has —
# archive state, deleted origins, folder children, and the `daily_queue`
# binding. A bare `schemes.remove()` clears only the map, and a `daily_queue`
# entry left pointing at the removed id re-materializes that day as an EMPTY
# page on the next sync, which then syncs out as authoritative content loss.
#
# Nothing downstream can repair it: a Daily page merely outside the loaded
# window is ALSO absent from `schemes` (desktop loads only up to today), so
# "absent scheme" cannot distinguish removed from unloaded, and pruning on
# absence would delete real days. The invariant has to hold where removal is
# KNOWN — here.
#
# Test code is exempt: several tests deliberately construct damaged or lazily
# unloaded workspaces (see `lazy_loaded` in crdt/tests/workspace_materialization.rs).
set -euo pipefail

cd "$(dirname "$0")/../.."

python3 - <<'PY'
import pathlib, re, sys

# The only production sites allowed to call `schemes.remove()` directly.
#   archive.rs       - the chokepoint's own implementation
#   sync_identity.rs - the daily-id RE-KEY: removes and re-inserts under the
#                      derived id, maintaining daily_queue/scheme_sync itself.
#                      Routing it through the chokepoint would delete the very
#                      binding it exists to repair.
ALLOWED = {
    "shared/model/src/workspace/archive.rs",
    "shared/model/src/workspace/sync_identity.rs",
}

# Match the actual `schemes` map, not a longer identifier such as
# `recent_moved_item_landed_schemes`. The latter is unrelated bookkeeping and
# must not be mistaken for removal of a workspace scheme.
CALL = re.compile(r"(?<![A-Za-z0-9_])schemes\s*\.\s*remove\s*\(")
offenders = []

for root in ("shared", "desktop", "tools"):
    for path in pathlib.Path(root).rglob("*.rs"):
        rel = path.as_posix()
        # Whole-file test exemptions: a tests/ directory, or a file that IS tests.
        if "/tests/" in f"/{rel}" or path.name in ("tests.rs",) or path.name.endswith("_tests.rs"):
            continue
        if rel in ALLOWED:
            continue
        try:
            lines = path.read_text(encoding="utf-8").splitlines()
        except (UnicodeDecodeError, OSError):
            continue
        # Anything after a file's first `#[cfg(test)]` is test code. Rust
        # convention puts that block last, so this is a sound cut-off and avoids
        # allowlisting whole production files just because they carry tests.
        cutoff = len(lines)
        for i, line in enumerate(lines):
            if line.strip().startswith("#[cfg(test)]"):
                cutoff = i
                break
        for i, line in enumerate(lines[:cutoff], start=1):
            stripped = line.strip()
            if stripped.startswith("//"):
                continue
            if CALL.search(line):
                offenders.append(f"{rel}:{i}: {stripped}")

if offenders:
    print("error: schemes.remove() outside the removal chokepoint:", file=sys.stderr)
    for offender in offenders:
        print(f"  {offender}", file=sys.stderr)
    print("", file=sys.stderr)
    print("Use Workspace::remove_scheme_completely(id) — it also clears the", file=sys.stderr)
    print("daily_queue binding, which otherwise outlives the scheme and", file=sys.stderr)
    print("re-materializes that day as an empty page.", file=sys.stderr)
    raise SystemExit(1)

print("scheme removal chokepoint: clean")
PY
