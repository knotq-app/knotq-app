#!/usr/bin/env bash
#
# run-sync-coverage.sh — line coverage for everything on the sync path.
#
# Runs every in-process sync suite under cargo-llvm-cov — the shared engine's
# tests and property fuzzer, the desktop state/commands/storage tests, and the
# desktop production-path fuzzer and scenarios — then reports line coverage for
# the sync-relevant source files and fails when any of them falls below its
# floor. The backend-driven HTTP/WebSocket suites are not included: they need a
# `wrangler dev` backend and are covered by run-sync-stress.sh.
#
# Usage (from app/):
#   ./.github/scripts/run-sync-coverage.sh             # measure and enforce
#   KNOTQ_COVERAGE_FLOOR=85 ./.github/scripts/run-sync-coverage.sh
#   KNOTQ_COVERAGE_HTML=1 ./.github/scripts/run-sync-coverage.sh   # + HTML report
#
# Requires: cargo-llvm-cov (`cargo install cargo-llvm-cov`) and the
# llvm-tools-preview rustup component, plus python3.

set -euo pipefail

cd "$(dirname "$0")/../.."

FLOOR="${KNOTQ_COVERAGE_FLOOR:-70}"
OUT_DIR="target/sync-coverage"
mkdir -p "${OUT_DIR}"

# Modest fuzz depth: coverage wants breadth of paths, not seed count.
export KNOTQ_FUZZ_SEEDS="${KNOTQ_FUZZ_SEEDS:-4}"
export KNOTQ_FUZZ_STEPS="${KNOTQ_FUZZ_STEPS:-120}"

echo "run-sync-coverage: cleaning previous profiles…"
cargo llvm-cov clean --workspace

# A failing suite must not hide the coverage numbers: record it, report, and
# fail at the end.
SUITES_FAILED=0

echo "run-sync-coverage: shared sync engine (unit, regression, property fuzz)…"
cargo llvm-cov --no-report -p knotq-sync --lib --tests || SUITES_FAILED=1

echo "run-sync-coverage: commands, state, storage…"
cargo llvm-cov --no-report -p knotq-commands -p knotq-state -p knotq-storage-json || SUITES_FAILED=1

echo "run-sync-coverage: desktop sync service + production-path fuzzer…"
cargo llvm-cov --no-report -p knotq-app -- sync_service || SUITES_FAILED=1

echo "run-sync-coverage: building report…"
cargo llvm-cov report --json --output-path "${OUT_DIR}/coverage.json"
if [ -n "${KNOTQ_COVERAGE_HTML:-}" ]; then
  cargo llvm-cov report --html --output-dir "${OUT_DIR}"
fi

COVERAGE_FAILED=0
python3 - "${OUT_DIR}/coverage.json" "${FLOOR}" <<'PY' || COVERAGE_FAILED=1
import json
import os
import sys

path, floor = sys.argv[1], float(sys.argv[2])
repo = os.getcwd() + os.sep

# The sync path: the shared engine, the command layer every edit goes through,
# the store that turns edits into CRDT updates, the on-disk formats sync state
# lives in, and the desktop sync service that runs it all.
INCLUDE = (
    "shared/sync/src/",
    "desktop/commands/src/",
    "desktop/state/src/store.rs",
    "desktop/state/src/state.rs",
    "desktop/state/src/daily_queue.rs",
    "desktop/state/src/workspace_view.rs",
    "desktop/storage-json/src/crdt_state.rs",
    "desktop/storage-json/src/sync_state.rs",
    "desktop/storage-json/src/files.rs",
    "desktop/storage-json/src/schema.rs",
    "desktop/app/src/app/sync_service/",
    "shared/model/src/workspace/",
)
EXCLUDE = (
    # Test-only code measured against itself.
    "desktop/app/src/app/sync_service/production_fuzz/",
    "shared/sync/src/testing.rs",
    # The live socket needs a real backend (run-sync-stress.sh covers it).
    "desktop/app/src/app/sync_service/ws_socket.rs",
    "shared/sync/src/ws/",
)

with open(path) as handle:
    data = json.load(handle)

rows = []
for export in data["data"]:
    for entry in export["files"]:
        name = entry["filename"]
        rel = name[len(repo):] if name.startswith(repo) else name
        if not rel.startswith(INCLUDE) or rel.startswith(EXCLUDE):
            continue
        lines = entry["summary"]["lines"]
        if lines["count"] == 0:
            continue
        rows.append((lines["percent"], lines["covered"], lines["count"], rel))

rows.sort()
total_covered = sum(row[1] for row in rows)
total_count = sum(row[2] for row in rows)
below = [row for row in rows if row[0] < floor]

print(f"\n{'lines':>8} {'covered':>9}  file")
for percent, covered, count, rel in rows:
    marker = "  <-- below floor" if percent < floor else ""
    print(f"{percent:7.1f}% {covered:>4}/{count:<4}  {rel}{marker}")
overall = 100.0 * total_covered / max(total_count, 1)
print(f"\nsync path overall: {overall:.1f}% ({total_covered}/{total_count} lines), floor {floor:.0f}% per file")

if below:
    print(f"run-sync-coverage: {len(below)} file(s) below {floor:.0f}% line coverage", file=sys.stderr)
    sys.exit(1)
print("run-sync-coverage: coverage floor met")
PY

if [ "${SUITES_FAILED}" -ne 0 ]; then
  echo "run-sync-coverage: FAILED — one or more test suites failed (see output above)" >&2
  exit 1
fi
if [ "${COVERAGE_FAILED}" -ne 0 ]; then
  echo "run-sync-coverage: FAILED — coverage below floor" >&2
  exit 1
fi
echo "run-sync-coverage: PASSED"
