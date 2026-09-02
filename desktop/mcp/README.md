# KnotQ MCP server

Lets an AI assistant read and edit a KnotQ workspace over the [Model Context
Protocol](https://modelcontextprotocol.io).

## Shape of the thing

The server runs **inside the desktop app**, not as a separate process and not in
the cloud. That is a correctness requirement, not a convenience:

- Every write goes through `KnotQApp::apply` on the GPUI main thread — the same
  call a keystroke makes. That is where invariants run, where the undo entry is
  pushed, where notifications are reconciled, and where the CRDT edit is
  recorded.
- A second process editing the same files would produce exactly the divergence
  the CRDT design exists to prevent, and its edits would not undo or reschedule
  notifications.

Consequences worth knowing up front: the assistant can only reach the workspace
while KnotQ is running, and it sees the local workspace — including anything not
yet synced.

```
  client ──stdio──▶ knotq-mcp ──HTTP──▶ KnotQ.app ──▶ apply() ──▶ CRDT / undo / notifications
                     (bridge)          127.0.0.1
```

## Crates

| Path | What it is |
|---|---|
| `desktop/mcp` | The tool surface, pure. Takes a workspace and a tool call, returns JSON or a `Command`. No HTTP, no GPUI, no I/O. |
| `desktop/app/src/app/mcp_service` | Transport: the loopback listener, admission checks, and the hop to the main thread. |
| `tools/mcp-bridge` | `knotq-mcp`, a stdio↔HTTP shim for clients that only launch subprocesses. |

The split is what makes the surface testable: every tool, and every mapping from
tool call to command, is covered without a window or a socket.

## Turning it on

There is no settings UI yet. In `settings.json` in the [data
directory](../../CLAUDE.md):

```json
"mcp": { "enabled": true, "read_only": false, "port": 52373 }
```

Restart KnotQ. It writes `mcp-endpoint.json` beside `settings.json`:

```json
{
  "url": "http://127.0.0.1:52373/mcp",
  "port": 52373,
  "token": "…",
  "read_only": false,
  "protocol_version": "2025-06-18"
}
```

The server is up for the whole session, so a client's saved configuration
survives a restart. If the configured port is taken it binds another one and
records it here — read the file, don't assume the port.

Point a stdio client at the bridge:

```json
{ "mcpServers": { "knotq": { "command": "knotq-mcp" } } }
```

Or POST straight to the URL with `Authorization: Bearer <token>`.

## Security

- **Off by default.** Enabling opens a port any local process can reach.
- **Loopback only** — bound to `127.0.0.1`, never `0.0.0.0`.
- **Bearer token**, regenerated on every start, stored `0600` in the data
  directory. A token that leaked stops working when the app restarts.
- **Origin checked before the token.** A web page can be made to resolve a
  hostname to `127.0.0.1` and POST here from the user's own browser; the
  `Origin` header is the only thing that distinguishes it, so a non-loopback
  origin is refused outright. This is the DNS-rebinding defence the MCP spec
  requires of local HTTP servers.
- **`read_only`** refuses every write tool independently of `enabled`, so
  reading can be granted without granting writing.
- Body size, read, write and evaluation are all bounded, so a local process
  cannot wedge the app by going quiet mid-request.

## Tools

Reads: `list_schemes`, `read_scheme`, `search`, `list_upcoming`, `list_overdue`,
`list_calendar`, `get_daily_queue`.

Writes: `create_scheme`, `rename_scheme`, `delete_scheme`, `create_folder`,
`rename_folder`, `delete_folder`, `add_item`, `update_item`, `delete_item`,
`move_item`, `set_item_completed`, `set_item_recurrence`.

Deliberately **not** exposed:

- `PermanentlyDelete*` — an agent must not be able to make an unrecoverable
  deletion on the user's behalf. `delete_*` archives, which the user can undo.
- `SetSchemeGsync` / `SetSchemeSource` — these rewire a scheme's relationship
  with an external calendar, which is account configuration, not planning.
- `undo` — the undo stack belongs to the person at the keyboard. An agent that
  could pop it could erase the user's own work.

### Properties the tools hold

- **Ids are opaque.** Pass back exactly what a read tool returned. Occurrence
  handles in particular encode a zoned original start; a handle the agent builds
  itself is refused rather than guessed at.
- **Retries are safe.** `add_item` takes an optional client-chosen `item_id`, and
  a replay with the same id is a no-op. `set_item_completed` is a *set*, not a
  toggle, so asking twice does not un-complete the task.
- **Absent ≠ null.** On `update_item`, omitting a field leaves it alone; passing
  it as `null` clears it.
- **Refusals reach the model.** A read-only scheme or a stale id comes back as a
  successful call with `isError: true`, which the model can read and adapt to —
  not a JSON-RPC error, which most clients swallow before the model sees it.

## Tests

| Where | What it covers |
|---|---|
| `desktop/mcp/tests/read_tools.rs` | Every read tool, ordering stability, clamping, refusals. |
| `desktop/mcp/tests/write_tools.rs` | The command each tool emits, round trips, idempotency, refusals. |
| `desktop/mcp/tests/protocol.rs` | JSON-RPC framing, result wrapping, and that the advertised surface matches the implemented one. |
| `desktop/app/src/app/mcp_service/http.rs` | Admission: paths, methods, tokens, origins. |
| `desktop/app/src/app/mcp_service/server.rs` | The real listener over real sockets: split bodies, concurrent clients, port fallback. |
| `shared/sync/tests/mcp_agent_convergence.rs` | That agent edits converge with human ones, across devices, restarts and retries. |
| `desktop/mcp/tests/manual_e2e.sh` | Manual, not run by `cargo test`: a real KnotQ driven over a real socket, to prove the app wires the server up at all. |

The convergence file is the one that matters most. The CRDT is only commutative
if every writer feeds it the same way, and a new writer is exactly the kind of
thing that quietly grows its own path.
