#!/usr/bin/env bash
# ---------------------------------------------------------------------------
# run-sync-stress.sh — the sync-convergence stress gate.
#
# Starts the KnotQ backend in test mode (`wrangler dev`) on an isolated state
# dir, waits for it, runs the Rust HTTP + WebSocket scenario/fuzz suites against
# it, then tears it down. Used by CI (`.github/actions/sync-stress`) and by hand
# before any deployment — see AGENTS.md / CLAUDE.md "Deployment gate".
#
# Run from the Rust workspace root (`app/`). Requires the backend checked out at
# `backend/` (a separate repo, git-ignored here) with `pnpm install` done, and
# `wrangler` / `pnpm` / `cargo` / `curl` on PATH.
#
#   ./.github/scripts/run-sync-stress.sh              # scenario suites
#   ./.github/scripts/run-sync-stress.sh --fuzz       # + deeper WS fuzz
#
# Depth knobs: KNOTQ_WS_FUZZ_SEEDS, KNOTQ_WS_FUZZ_STEPS.
# ---------------------------------------------------------------------------
set -euo pipefail

PORT="${KNOTQ_STRESS_PORT:-8788}"
BACKEND_URL="http://127.0.0.1:${PORT}"
BACKEND_DIR="${KNOTQ_BACKEND_DIR:-backend/cloudflare}"
PERSIST=".wrangler/integration-test-state"

RUN_FUZZ=0
[ "${1:-}" = "--fuzz" ] && RUN_FUZZ=1

# The scenario suites all hammer one `wrangler dev` instance. On a small CI
# runner (2 vCPU) the default one-thread-per-core fan-out overwhelms workerd and
# it drops connections mid-request; cap concurrency there. Empty locally =
# cargo's default.
TEST_THREADS="${KNOTQ_STRESS_TEST_THREADS:-}"
THREAD_ARG=""
[ -n "${TEST_THREADS}" ] && THREAD_ARG="--test-threads=${TEST_THREADS}"

if [ ! -d "${BACKEND_DIR}" ]; then
  echo "run-sync-stress: ${BACKEND_DIR} not found — check out knotq-app/backend there." >&2
  exit 1
fi

# Compile the test binaries BEFORE starting wrangler dev. rustc + the linker peak
# at several GB; on a 2-vCPU / 7 GB CI runner that memory spike gets `workerd`
# OOM-killed while it sits idle waiting, and every test then fails instantly
# against a dead backend.
echo "run-sync-stress: pre-building test binaries…"
cargo test -p knotq-sync --test backend_integration --test ws_integration --no-run

WRANGLER_PID=""
cleanup() {
  if [ -n "${WRANGLER_PID}" ]; then
    kill "${WRANGLER_PID}" 2>/dev/null || true
    for _ in 1 2 3 4 5; do kill -0 "${WRANGLER_PID}" 2>/dev/null || break; sleep 1; done
    kill -9 "${WRANGLER_PID}" 2>/dev/null || true
  fi
}
trap cleanup EXIT INT TERM

echo "run-sync-stress: applying D1 migrations…"
( cd "${BACKEND_DIR}" && CI=1 pnpm wrangler d1 migrations apply knotq-auth \
    --local --persist-to "${PERSIST}" )

echo "run-sync-stress: starting wrangler dev on :${PORT} (KNOTQ_TEST_MODE=1)…"
( cd "${BACKEND_DIR}" && pnpm wrangler dev --local --port "${PORT}" \
    --var KNOTQ_TEST_MODE:1 --persist-to "${PERSIST}" --log-level warn ) &
WRANGLER_PID=$!

for attempt in $(seq 1 60); do
  if curl -sf "${BACKEND_URL}/healthz" >/dev/null 2>&1; then
    echo "run-sync-stress: backend ready after ${attempt} attempts."; break
  fi
  kill -0 "${WRANGLER_PID}" 2>/dev/null || { echo "run-sync-stress: wrangler exited early" >&2; exit 1; }
  sleep 0.5
done
curl -sf "${BACKEND_URL}/healthz" >/dev/null || { echo "run-sync-stress: backend never became ready" >&2; exit 1; }

# Prove the DB schema is actually present before handing off to the suite: a
# migration that silently applied nothing (wrong persist path, non-interactive
# abort) shows up here as a bootstrap 500 instead of 26 identical failures.
probe_email="stress-probe-$(date +%s)@example.com"
probe_status="$(curl -s -o /dev/null -w '%{http_code}' -X POST \
  -H 'content-type: application/json' -d "{\"email\":\"${probe_email}\"}" \
  "${BACKEND_URL}/__test/bootstrap")"
if [ "${probe_status}" != "200" ]; then
  echo "run-sync-stress: /__test/bootstrap probe returned ${probe_status} — D1 not ready" >&2
  ( cd "${BACKEND_DIR}" && pnpm wrangler d1 execute knotq-auth --local \
      --persist-to "${PERSIST}" --command \
      "SELECT name FROM sqlite_master WHERE type='table'" 2>&1 || true )
  exit 1
fi
echo "run-sync-stress: /__test/bootstrap probe ok."

export KNOTQ_SYNC_BACKEND_URL="${BACKEND_URL}"

echo ""
echo "run-sync-stress: HTTP scenario suite…"
cargo test -p knotq-sync --test backend_integration -- --nocapture ${THREAD_ARG}

echo ""
echo "run-sync-stress: WebSocket scenario suite…"
# The fixed scenarios (2-device convergence, presence, changed-broadcast,
# account switch, g/g2/e/f/m2). The randomized/fuzz tests run below.
cargo test -p knotq-sync --test ws_integration -- --nocapture ${THREAD_ARG} \
  --skip _fuzz --skip hopping

echo ""
echo "run-sync-stress: WebSocket + mixed-transport + account-hopping fuzz…"
# WS-only, WS/HTTP-alternating-per-request, and account-hopping — all over the
# real socket against the real worker.
KNOTQ_WS_FUZZ_SEEDS="${KNOTQ_WS_FUZZ_SEEDS:-$([ "${RUN_FUZZ}" -eq 1 ] && echo 8 || echo 3)}" \
KNOTQ_WS_FUZZ_STEPS="${KNOTQ_WS_FUZZ_STEPS:-$([ "${RUN_FUZZ}" -eq 1 ] && echo 60 || echo 30)}" \
KNOTQ_WS_HOP_SEEDS="${KNOTQ_WS_HOP_SEEDS:-$([ "${RUN_FUZZ}" -eq 1 ] && echo 3 || echo 1)}" \
KNOTQ_WS_HOP_STEPS="${KNOTQ_WS_HOP_STEPS:-$([ "${RUN_FUZZ}" -eq 1 ] && echo 24 || echo 16)}" \
  cargo test -p knotq-sync --test ws_integration -- --nocapture ${THREAD_ARG} \
    ws_scenario_l_randomized_fuzz \
    ws_scenario_l_randomized_fuzz_mixed_transport \
    ws_account_hopping_fuzz_converges

echo ""
echo "run-sync-stress: PASSED."
