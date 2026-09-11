# KnotQ agent knowledge base

This is the short operational map for future agents. It is intentionally
separate from skills: read it when onboarding, even if no skill is selected.

## Repository map

- `app/` is the desktop/shared/backend parent repository.
- `app/mobile/` is a separate Git repository containing the iOS/Android shells
  and mobile Rust core. Always inspect both Git roots.
- Desktop source is under `desktop/`; shared model/sync is under `shared/`.
- The backend lives in a separate checkout at `app/backend/cloudflare/` during
  local Wrangler testing and is not necessarily present.

## First five minutes

```sh
git -C app status --short
git -C app/mobile status --short
git -C app log --oneline -20
git -C app/mobile log --oneline -20
rg --files app/.github app/mobile/.github | sort
```

Read the nearest `AGENTS.md`/`CLAUDE.md` before editing. Preserve dirty files;
they may be work from another agent or an unreleased fix.

## Non-negotiable safety

- Do not deploy desktop, backend, website, iOS, or Android without explicit
  permission.
- Do not change persisted formats outside `desktop/storage-json/src/upgrade/`.
- Do not advance sync cursors past an unverified document or call convergence
  green when a transport suite early-exited.
- Never use destructive Git commands on a dirty root.
- Treat store counts and framework-only crash frames as impact evidence, not a
  root cause.

## Where to look

| Question | Start here |
| --- | --- |
| Sync divergence | `shared/sync/src`, `shared/sync/tests`, `mobile/core/src/tests.rs` |
| Deferred Daily Queue | `shared/sync/src/engine.rs`, `mobile/core/src/mobile_core_inner_ops.rs` |
| Calendar/search freshness | `desktop/index`, `mobile/core/src/mobile_core_inner_views.rs` |
| Mobile Rust test organization | `mobile/core/src/tests.rs`, `tests_daily.rs`, `tests_calendar.rs`, `tests_archive.rs`, `tests_notifications.rs`, `tests_more.rs` |
| iOS editor glitches | `mobile/ios/KnotQMobile/SchemeEditor*`, `RemoteMergeTests.swift` |
| Mobile auth/notifications/crashes | [`mobile-operations.md`](mobile-operations.md) |
| Android lifecycle | `mobile/android/app/src/main/kotlin/com/enigmadux/knotq/*Sync*` |
| MCP | `desktop/mcp`, `desktop/app/src/app/mcp_service`, `shared/sync/tests/mcp_agent_convergence.rs` |
| Backend security/performance | `backend/cloudflare/src/shared/helpers`, D1 migrations, `backend-operations.md` |
| Performance iteration | [`performance.md`](performance.md), the performance-loop skill, existing perf budgets |
| Validation ladder | [verification.md](verification.md), [test-ladder skill](../../.agents/skills/test-ladder/SKILL.md) |
| Release/crash evidence | `.agents/skills/`, `.github/workflows/`, platform archives/logs |

The reusable [backend-operations skill](../../.agents/skills/backend-operations/SKILL.md)
covers the bounded provider, readiness, security-header, request-correlation,
and no-deploy checks for backend work.

See [history.md](history.md) for recurring failures and [verification.md](verification.md)
for the test ladder.
