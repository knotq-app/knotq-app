# Backend operations and diagnostics

The backend checkout is normally at `app/backend/cloudflare/` and is a
separate Git root. Inspect its status before editing; preserve unrelated dirty
work. Do not deploy from a test/debugging pass.

## Verification ladder

```sh
cd app/backend/cloudflare
pnpm run typecheck
pnpm run validate:deployment-config
pnpm run test:monitoring
pnpm test -- --reporter=dot
pnpm audit --prod --audit-level=moderate
```

The package test wrapper accepts forwarded Vitest arguments. For example,
`pnpm test -- test/push.test.ts --maxWorkers=1` really filters to that file.
The backend tests run in Workers isolates and Argon2 can make parallel runs
slow; a targeted single-worker run is the deterministic first diagnosis for a
failure. A full pass is still required before backend release work.

The Workers test project intentionally sets both `isolatedStorage: false` and
`singleWorker: true`: tests use unique identifiers while exercising shared D1 and
Durable Object state, and one module worker avoids per-file loopback fallback
connection churn that can exhaust macOS ephemeral ports.

For transport correctness, from `app/` run:

```sh
./.github/scripts/run-sync-stress.sh --fuzz
```

This starts a local Wrangler Worker and exercises HTTP, real WebSockets, mixed
transport, account hopping, media, notification schedules, and Daily Queue
scenarios. A process that exits before the HTTP/WS/fuzz summaries is not a
green result.

The backend repository's `.github/workflows/ci.yml` runs the same typecheck and
Workers/D1 suite on every push and pull request, plus the production dependency
audit and a real HTTP/WebSocket sync-contract job. That job mounts the backend
checkout under review into the canonical app harness, so backend-only changes
are not validated solely against mocked/in-process Worker requests. Keep that
workflow independent from deployment workflows: green CI does not publish a
Worker.

`.github/workflows/codeql.yml` adds scheduled and change-triggered CodeQL
analysis for the Worker TypeScript and GitHub Actions workflow code. It uses
read-only source access; only the analysis job receives `security-events: write`
so build/test jobs cannot mutate repository security state.

## Security and observability invariants

- Dynamic JSON responses are `no-store` and `nosniff`; user media remains
  private and attachment-forced.
- The live `/v1/sync/ws` upgrade response must retain `cache-control: no-store`,
  the standard security headers, and its bounded `x-request-id`. The top-level
  request wrapper cannot safely rebuild a `101` response after the WebSocket is
  attached, so `workspace_object/socket.ts` applies these headers on the live
  handshake itself. Keep the integration assertion when changing upgrade code.
- Every public non-WebSocket response has an `x-request-id`. Include it when
  reporting a production failure so request and Worker logs can be correlated.
- `/healthz` is dependency-free liveness; `/readyz` performs a minimal D1 read
  and is the correct uptime/readiness probe for detecting auth-database outages.
- `pnpm run test:monitoring` covers the dependency-free synthetic checker;
  `node scripts/synthetic-health.mjs` probes both configured public targets.
  Synthetic targets must be HTTPS host roots (no path, credentials, query, or
  fragment), and a failing production target does not hide the sandbox result;
  the scheduled log reports each target independently before exiting non-zero.
  The probe intentionally requires `/readyz`, `x-request-id`, `nosniff`, a
  restrictive CSP, and one-year HSTS, so it will flag an older deployment that
  predates the current observability/security contract instead of reporting it
  as fully healthy.
- Unexpected exceptions emit `request.unhandled_error`; slow (at least one
  second) and 5xx requests emit `request.slow_or_failed`. Warn/error events are
  retained; high-volume sync events carry a sampling rate.
- `errorLogMessage` caps exception text at 500 characters and redacts bearer,
  access/refresh-token, and credential-like query text before it is included in
  any structured error log. `logEvent` also redacts sensitive field names
  recursively while preserving request/sweep IDs and ordinary provider detail
  for correlation.
- Rate-limit logs retain only a scope, never the raw IP/user/email-derived key.
- Per-IP limiter keys use only Cloudflare's authoritative `CF-Connecting-IP`;
  `X-Forwarded-For` is intentionally ignored because it is spoofable at an
  untrusted edge/preview.
- The public analytics endpoint has its own `RL_ANALYTICS_IP` budget; do not
  reuse an auth limiter for telemetry, because a traffic spike must not consume
  sign-in capacity. The production and sandbox binding maps are checked for
  completeness and namespace isolation by `validate:deployment-config`.
- Pull traffic has a separate `RL_WORKSPACE_PULL` budget because it is the
  normal high-frequency path. Device management, compaction, and history-squash
  remain under the lower `RL_WORKSPACE` budget; multi-device polling must not
  starve those controls.
- The partial auth indexes in `migrations/0007_hot_path_indexes.sql` are part of
  the runtime performance contract, not just a migration artifact. The
  `schema-hot-paths.test.ts` test checks both their presence and SQLite's query
  plan for entitlement and active-session lookups; update that test with any
  query-shape change before adding or removing an index.
- All JSON, webhook, and other raw request bodies are bounded by byte length;
  missing `Content-Length` is not a reason to skip reading/limiting a chunked
  body. Keep `request-limits.test.ts` when changing request parsing.
- Bodies returned by Google, Lemon Squeezy, FCM, and GitHub are also streamed
  through bounded readers before JSON/error parsing. A provider response above
  its endpoint ceiling is an upstream 502, not a client error or an unbounded
  allocation.
- External provider calls use `fetchWithTimeout` with a bounded default
  deadline. Preserve that wrapper when adding integrations, and keep provider
  error bodies bounded as well; an unavailable provider must not pin a Worker
  request indefinitely.
- Auth tokens, refresh secrets, push tokens, private keys, and webhook bodies
  must not be added to logs or admin responses.
- The Vitest worker harness generates ephemeral PASETO and RSA credentials at
  process startup (`cloudflare/vitest.config.ts`); never paste a provider key
  into test configuration, even when the provider is mocked. Rotation tests
  read the generated current/previous keys from the worker environment.
- PKCE authorization codes are read first, verified against the app's verifier,
  then atomically compare-and-deleted. Never consume a code before PKCE
  validation: an intercepted redirect code must be unable to burn a legitimate
  app exchange. Keep the wrong-verifier and concurrent-exchange tests in
  `test/authorize.test.ts`.
- CORS is an explicit origin allowlist. Test both an allowed and attacker
  origin, and assert `Vary: Origin`.
- `pnpm run validate:deployment-config` checks that production and sandbox use
  separate routes, Worker names, D1/R2 stores, HTTPS-only allowlists, enabled
  observability, and no test-only flags before a release workflow can proceed.
  It also rejects wildcard, malformed, credential-bearing, path-bearing, and
  production-localhost CORS origins; an origin entry must be a bare URL origin.

## Production stop points

Before any backend deploy, run the full sync stress gate, backend typecheck,
and full backend tests. Review D1 migrations (append-only), bindings, secret
names, CORS origins, rate limiting/WAF configuration, and Wrangler
observability settings. Stop before `wrangler deploy` unless the user has
explicitly authorized publication.
