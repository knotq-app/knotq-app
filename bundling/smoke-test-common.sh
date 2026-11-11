#!/usr/bin/env bash
# Shared by the per-platform smoke tests. Source it; do not run it.

# smoke_launch <bin> <work-dir>
# Launches the packaged app against a throwaway home and data directory and
# fails unless it is still running after SMOKE_SECONDS and has written its
# first workspace. Catches what unit tests cannot: a dylib or rpath the bundle
# does not carry, a missing asset, a signing or entitlement problem that kills
# the process, a panic during startup.
smoke_launch() {
    local bin="$1" work="$2" secs="${SMOKE_SECONDS:-20}"
    local data="$work/data" log="$work/app.log"
    mkdir -p "$work/home" "$data"
    test -x "$bin" || { echo "[error] not an executable: $bin" >&2; return 1; }

    echo "[smoke] launching $bin for ${secs}s"
    HOME="$work/home" KNOTQ_DATA_DIR="$data" "$bin" >"$log" 2>&1 &
    local pid=$!
    sleep "$secs"
    if ! kill -0 "$pid" 2>/dev/null; then
        local rc=0
        wait "$pid" || rc=$?
        echo "[error] the app exited with status $rc during launch" >&2
        cat "$log" >&2
        return 1
    fi
    kill "$pid"
    wait "$pid" 2>/dev/null || true

    # Alive is not enough: an app stuck before bootstrap is alive too.
    if [[ ! -s "$data/workspace/workspace.json" ]]; then
        echo "[error] the app stayed up but never wrote its workspace" >&2
        find "$data" -maxdepth 2 >&2 || true
        cat "$log" >&2
        return 1
    fi
    echo "[smoke] app stayed up and wrote its workspace"
}
