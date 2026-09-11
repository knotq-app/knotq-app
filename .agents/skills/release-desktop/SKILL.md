---
name: release-desktop
description: Build and verify KnotQ macOS, Windows, or Linux release artifacts without publishing them.
---

Use for desktop release preparation. A release is not complete until the sync
stress gate, platform build, packaging checks, and artifact inspection pass.

- Run the full sync/property gate before a tag or distributable build. The
  Wrangler-backed suite requires the separate backend checkout and its token.
- macOS: use `local/run-app.sh` for a real debug smoke check; for a release
  artifact verify bundle ID/version, embedded MCP bridge, ad-hoc or configured
  signature, and notarization only when credentials are intentionally supplied.
- Windows/Linux: build the exact target, run installer/layout checks, inspect
  executable architecture, and verify checksums.
- Keep build output and logs outside the source tree when possible. Do not run
  release-upload, GitHub-release, notarization, or store/publisher steps unless
  explicitly requested.
- Preserve dirty worktrees and distinguish a build failure from a missing
  platform dependency or signing secret.
