---
name: worktree-workflow
description: Safely create, inspect, and clean up KnotQ worktrees across the nested desktop and mobile repositories.
---

Use before parallel agent work, release preparation, or risky refactors.

1. Find every Git root and record branch, HEAD, worktrees, submodules/path
   dependencies, and dirty files. The mobile checkout is intentionally a
   separate repository under `app/mobile`.
2. Create a named worktree from the correct repository and branch. Keep each
   agent's write scope disjoint; never share a dirty worktree for independent
   edits.
3. Build from the worktree's real workspace root so path dependencies resolve.
   Regenerated Swift/Kotlin bindings and build outputs are disposable only when
   they are confirmed generated; user edits are not.
4. Before handoff, run targeted tests, `git diff --check`, and status in every
   affected root. Report commits/paths and leave cleanup to an explicit,
   validated operation. Never use `reset --hard` or broad recursive deletion.
