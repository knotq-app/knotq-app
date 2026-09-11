---
name: sync-debugging
description: Diagnose KnotQ sync, deferred Daily Queue, wakeup, account, and cross-device convergence failures.
---

Trace the complete path before changing code: local command → CRDT capture →
pending queue → push/pull cursor → workspace materialization → persistence →
index/notification rebuild → UI publication.

- Snapshot both `app/` and `app/mobile/` Git roots. Reproduce with a fixed seed
  and preserve the smallest failing scenario.
- Run the in-memory property model and mobile-core accounts suite first, then
  the ignored multi-origin Daily Queue stress. Run HTTP/WebSocket suites against
  local Wrangler when `backend/cloudflare` is present; an early-return or absent
  backend is a skipped transport check, not a pass.
- Compare content, IDs, tombstones, pending edits, cursors, CRDT bytes, and
  materialized workspace after every round and after restart. Convergence alone
  is insufficient if every device lost the same content.
- Exercise account switching, logout/entitlement changes, foreground/background
  wakeups, stale socket replacement, deferred off-window dailies, carryover,
  recurrence exceptions, external calendar imports, and media failures.
- Do not weaken gates, reset user data, or advance a cursor past an unverified
  document. Add the failure to an existing scenario/fuzzer rather than a
  one-off test when possible.
