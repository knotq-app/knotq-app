---
name: knotq-desktop-release
description: Prepare and tag a KnotQ desktop release only after the repository's required validation gates pass.
---

Use this skill for desktop release branches, version tags, or shipping a desktop fix.

Work in a clean dedicated worktree based on `origin/main`. Keep unrelated dirty worktrees untouched. Review the diff against `origin/main`, run targeted tests and the full relevant desktop suite, then run `knotq-sync-release-gate` before tagging. Never tag a release with a red, skipped, or unverified mandatory sync gate.

Confirm the intended version in the desktop package metadata and release automation before creating an annotated tag. Verify the tag points at the tested commit, push the branch and tag explicitly, and report commit, tag, test commands, pass/fail results, and any unverified platform or signing boundary. Building locally does not prove a packaged artifact launched or that store/release automation succeeded.
