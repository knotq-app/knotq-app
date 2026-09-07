#!/bin/bash
# Manual end-to-end smoke test for the MCP server.
#
# Launches a real KnotQ against a THROWAWAY data directory and drives the
# server over a real socket: handshake, admission, reads, writes, an
# idempotent retry, and persistence to disk.
#
# Not run by `cargo test`, and not runnable in CI, because it needs a GUI
# process. Everything below the transport is covered by ordinary tests
# (`desktop/mcp/tests`, and the socket tests in
# `desktop/app/src/app/mcp_service/server.rs`); what only this can check is
# that the app *wires the server up* — that `start_if_enabled` reads the
# setting, binds, writes the endpoint file, and that a tool call reaches
# `KnotQApp::apply` and lands on disk.
#
#   cargo build -p knotq-app
#   desktop/mcp/tests/manual_e2e.sh target/debug/knotq
#
# NEVER point this at the real data directory. It sets KNOTQ_DATA_DIR to a
# scratch path; a dev build run against real data migrates
# sync-crdt-state.json and wedges the installed app.
set -u
SP="$(cd "$(dirname "$0")" && pwd)"
DATA="${TMPDIR:-/tmp}/knotq-mcp-e2e"
BIN="$1"
PORT=52999

rm -rf "$DATA"; mkdir -p "$DATA"
cat > "$DATA/settings.json" <<EOF
{"version":1,"settings":{"onboarding_completed":true,"mcp":{"enabled":true,"read_only":false,"port":$PORT}}}
EOF

export KNOTQ_DATA_DIR="$DATA"
"$BIN" > "$DATA/app.log" 2>&1 &
APP_PID=$!
echo "app pid $APP_PID, data dir $DATA"

# Wait for the endpoint descriptor rather than guessing at a startup delay.
for _ in $(seq 1 120); do
  [ -f "$DATA/mcp-endpoint.json" ] && break
  kill -0 $APP_PID 2>/dev/null || { echo "FAIL: app exited early"; cat "$DATA/app.log"; exit 1; }
  sleep 0.25
done
if [ ! -f "$DATA/mcp-endpoint.json" ]; then
  echo "FAIL: no endpoint file appeared"; cat "$DATA/app.log"; kill $APP_PID; exit 1
fi

URL=$(python3 -c "import json;print(json.load(open('$DATA/mcp-endpoint.json'))['url'])")
TOKEN=$(python3 -c "import json;print(json.load(open('$DATA/mcp-endpoint.json'))['token'])")
echo "endpoint $URL"

call() { curl -s -m 15 -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' -d "$1" "$URL"; }

fail=0
check() { # name, jq-ish python expr, json
  local name="$1" expr="$2" body="$3"
  if python3 -c "
import json,sys
d=json.loads(sys.stdin.read())
sys.exit(0 if ($expr) else 1)" <<< "$body"; then
    echo "  ok   $name"
  else
    echo "  FAIL $name -> $body"; fail=1
  fi
}

echo "--- handshake ---"
R=$(call '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}')
check "initialize names the server" "d['result']['serverInfo']['name']=='knotq'" "$R"

R=$(call '{"jsonrpc":"2.0","id":2,"method":"tools/list"}')
check "tools/list is non-empty" "len(d['result']['tools'])>=19" "$R"

echo "--- auth ---"
R=$(curl -s -m 10 -o /dev/null -w '%{http_code}' -H 'Content-Type: application/json' -d '{}' "$URL")
[ "$R" = "401" ] && echo "  ok   no token -> 401" || { echo "  FAIL no token -> $R"; fail=1; }
R=$(curl -s -m 10 -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $TOKEN" -H 'Origin: https://evil.example' -d '{}' "$URL")
[ "$R" = "403" ] && echo "  ok   web origin -> 403" || { echo "  FAIL web origin -> $R"; fail=1; }

echo "--- reads ---"
R=$(call '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_schemes","arguments":{}}}')
check "list_schemes returns the seeded workspace" "len(d['result']['structuredContent']['schemes'])>0" "$R"
SCHEME=$(python3 -c "
import json,sys
d=json.loads(sys.stdin.read())
print(d['result']['structuredContent']['schemes'][0]['id'])" <<< "$R")
echo "  scheme $SCHEME"

R=$(call "{\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"tools/call\",\"params\":{\"name\":\"read_scheme\",\"arguments\":{\"scheme_id\":\"$SCHEME\"}}}")
check "read_scheme returns items" "'items' in d['result']['structuredContent']" "$R"

R=$(call '{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"list_upcoming","arguments":{"limit":5}}}')
check "list_upcoming answers" "'occurrences' in d['result']['structuredContent']" "$R"

echo "--- writes ---"
R=$(call "{\"jsonrpc\":\"2.0\",\"id\":6,\"method\":\"tools/call\",\"params\":{\"name\":\"add_item\",\"arguments\":{\"scheme_id\":\"$SCHEME\",\"text\":\"written by the agent\",\"item_id\":\"11111111-2222-3333-4444-555555555555\"}}}")
check "add_item reports created" "d['result']['structuredContent']['created'] is True and d['result']['structuredContent']['changed'] is True" "$R"

R=$(call "{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"tools/call\",\"params\":{\"name\":\"read_scheme\",\"arguments\":{\"scheme_id\":\"$SCHEME\"}}}")
check "the line is really in the workspace" "any(i['text']=='written by the agent' for i in d['result']['structuredContent']['items'])" "$R"

# The retry an agent makes when a response is lost.
R=$(call "{\"jsonrpc\":\"2.0\",\"id\":8,\"method\":\"tools/call\",\"params\":{\"name\":\"add_item\",\"arguments\":{\"scheme_id\":\"$SCHEME\",\"text\":\"written by the agent\",\"item_id\":\"11111111-2222-3333-4444-555555555555\"}}}")
check "replaying add_item is a no-op" "d['result']['structuredContent']['created'] is False and d['result']['structuredContent']['changed'] is False" "$R"

R=$(call "{\"jsonrpc\":\"2.0\",\"id\":9,\"method\":\"tools/call\",\"params\":{\"name\":\"set_item_completed\",\"arguments\":{\"scheme_id\":\"$SCHEME\",\"item_id\":\"11111111-2222-3333-4444-555555555555\",\"completed\":true}}}")
check "set_item_completed applies" "d['result']['structuredContent']['completed'] is True" "$R"
R=$(call "{\"jsonrpc\":\"2.0\",\"id\":10,\"method\":\"tools/call\",\"params\":{\"name\":\"set_item_completed\",\"arguments\":{\"scheme_id\":\"$SCHEME\",\"item_id\":\"11111111-2222-3333-4444-555555555555\",\"completed\":true}}}")
check "asking again does not un-complete it" "d['result']['structuredContent']['changed'] is False" "$R"

echo "--- refusals reach the model ---"
R=$(call '{"jsonrpc":"2.0","id":11,"method":"tools/call","params":{"name":"read_scheme","arguments":{"scheme_id":"00000000-0000-0000-0000-000000000000"}}}')
check "a stale id is isError, not a transport failure" "d['result']['isError'] is True and 'error' not in d" "$R"

echo "--- persistence ---"
# Saves are debounced (SAVE_DEBOUNCE = 2s). Wait for the save task to fire
# rather than killing inside the window — SIGTERM does not run GPUI's
# on_app_quit hook, so an immediate kill proves nothing about whether the
# write signalled a save.
for _ in $(seq 1 40); do
  grep -rq "written by the agent" "$DATA/workspace" 2>/dev/null && break
  sleep 0.25
done
kill $APP_PID 2>/dev/null
for _ in $(seq 1 40); do kill -0 $APP_PID 2>/dev/null || break; sleep 0.25; done
if grep -rq "written by the agent" "$DATA" 2>/dev/null; then
  echo "  ok   the agent's line was written to disk"
else
  echo "  FAIL the agent's line never reached disk"; fail=1
fi

echo
[ $fail -eq 0 ] && echo "E2E PASSED" || echo "E2E FAILED"
exit $fail
