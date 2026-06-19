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
