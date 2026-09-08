## KnotQ App Workspace

Use the repository-level `../AGENTS.md` as the authoritative project guide.
This file exists to keep nested agent instructions aligned with the current app
workspace layout.

### Current Workspace Notes

- Desktop is a Rust + GPUI workspace under `desktop/`.
- Shared domain crates live under `shared/model` and `shared/sync`.
- Mobile lives in the separate `mobile/` Git root and uses the shared model/sync
  concepts through its own core bindings.
- Backend lives in the separate `backend/cloudflare/` Git root.
- There is no active `editor-core` crate.
- There is no active `fixtures` crate.
- The active theme list exported by `theme::all_themes()` is `Obsidian` and
  `Light`, even though additional source definitions may exist.

### Working Rules

- Preserve behavior unless the user explicitly requests a functional change.
- Keep desktop, shared, mobile, and backend boundaries clean; do not introduce
  dependency cycles or shared abstractions for superficially similar code.
- GPUI widgets must call `.id()` before `.on_click()`.
- Mirror `Cmd+*` shortcuts with `secondary-*` where the app already follows that
  cross-platform pattern.
- After merging a pull request, delete its remote branch (`git push origin
  --delete <branch>` or the hosting UI), but preserve the corresponding local
  branch/worktree. Do not delete a branch that still contains unmerged work,
  even if some of its commits were cherry-picked elsewhere.

### Deployment gate — run the sync stress suite before ANY deployment

**Before every deployment of any kind — a desktop release (tag `v*`), a backend
`wrangler deploy` / `deploy:sandbox`, or a mobile App Store / Play submission —
the full sync-convergence stress suite MUST pass.** A sync regression that
reverts a user's edits, wedges a device, or drops content is the highest-severity
class of bug this project has, and every one of them was found by fuzzing, not by
hand-testing.

The suite is one command, from `app/`:

```sh
./.github/scripts/run-sync-stress.sh --fuzz     # HTTP + WebSocket, against a real
                                           # wrangler dev backend it starts itself
KNOTQ_FUZZ_SEEDS=800 KNOTQ_FUZZ_STEPS=400 \
  cargo test -p knotq-sync --test sync_property_model --release   # in-memory, deep
```

CI enforces this: the `sync-stress` job runs on every PR (`ci.yml`), and the
`release.yml` build/sign jobs `needs: [sync-stress]` so no signed artifact is
produced until it is green. `sync-fuzz.yml` runs it deeper nightly. Do not
disable, `continue-on-error`, or weaken these gates. If the suite is red, the fix
is the sync bug, not the gate. (The wrangler-backed jobs need a
`BACKEND_REPO_TOKEN` secret with read access to `knotq-app/backend`.)

The harness is the real thing: `TestDevice` drives the production engine
(`batch_pull_and_apply` / `batch_push_pending`), `Harness::new_ws` runs every
scenario over the production `WsClient` → socket → Durable Object, and the mobile
`two_device_lazy_daily_lifecycle_fuzz` drives `MobileCoreInner::run_sync_cycle`
(the same method `sync_once` calls). When you touch sync, extend the fuzzer
rather than adding a bespoke reproduction.
