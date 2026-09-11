---
name: backend-operations
description: Diagnose and harden the KnotQ Cloudflare backend with bounded tests, readiness checks, security review, and correlated observability without deploying.
---

Use for backend incidents, security/performance reviews, provider failures, or
pre-release verification. The backend is a separate Git root at
`app/backend/cloudflare`; inspect its status before editing and preserve dirty
work.

## Safe diagnostic loop

1. Run the focused Vitest file with one worker, then rerun the full suite after
   the fix:

   ```sh
   cd app/backend/cloudflare
   pnpm exec vitest run test/<focused>.test.ts --no-file-parallelism --maxWorkers=1 --reporter=dot
   pnpm run typecheck
   pnpm run validate:deployment-config
   pnpm test -- --no-file-parallelism --maxWorkers=1 --reporter=dot
   pnpm audit --prod --audit-level=moderate
   ```

   The synthetic probe accepts HTTPS host roots only; it rejects paths,
   credentials, queries, and fragments, and reports production and sandbox
   failures independently before exiting non-zero.

2. For sync or Durable Object changes, run `app/.github/scripts/run-sync-stress.sh
   --fuzz` and inspect the HTTP, WebSocket, and mixed/account summaries. A
   wrapper that exits before all summaries is not green.

3. Correlate failures using `x-request-id`, timestamp, route, status, and the
   structured event name. `/healthz` is dependency-free liveness; `/readyz`
   checks D1 and is the readiness probe. Never paste bearer tokens, refresh
   tokens, push tokens, webhook bodies, raw IP keys, or full attacker payloads
   into logs or reports.

## Hardening checklist

- Keep request and provider-response bodies byte-bounded and external fetches
  deadline-bounded; add a regression test whenever a new integration is added.
- Preserve `no-store`, `nosniff`, CSP, HSTS, request IDs, explicit CORS, and
  non-WebSocket response wrapping.
- Treat auth, billing, media, and sync errors differently: terminal auth
  failures may invalidate a session, while provider/network failures must stay
  retryable and must not wedge a Durable Object.
- Review D1 changes as append-only migrations and inspect indexes against real
  hot queries. Do not modify production data as part of diagnosis.

## Stop point

Do not run `wrangler deploy`, change secrets, alter live D1/R2 data, or publish
an alert/dashboard configuration unless the user explicitly authorizes that
external mutation. Report unverified production boundaries separately from
local test evidence.
