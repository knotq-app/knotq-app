---
name: build-cache-cleanup
description: Reclaim disk from superseded KnotQ build caches without touching user data, submitted archives, or a build that is currently running.
---

Use when the machine is short on disk. KnotQ's build trees grow without bound:
`cargo` writes a fresh hashed artifact per build and never removes the one it
replaced, so a few weeks of work leaves tens of gigabytes of artifacts nothing
references any more.

Measured on the development machine, 2026-09-20, for a sense of scale and where
to look first:

| Path | Size | What it is |
|---|---|---|
| `app/target/debug/deps` | 25G | one artifact per build; ~half superseded within a day |
| `app/target/debug/incremental` | 8.1G | pure cache, regenerable |
| `~/Library/Developer/CoreSimulator/Devices` | 24G | simulator devices, incl. ones for uninstalled runtimes |
| `app/mobile/target` | 15G | host debug plus four cross targets |
| `~/Library/Developer/Xcode/DerivedData` | 3.0G | Xcode cache, regenerable |
| `~/.cargo/registry` | 1.8G | shared source cache |
| `~/.gradle/caches` | 822M | regenerable |

## Never delete these

- **The KnotQ data directory.** `~/Library/Application Support/KnotQ` on macOS,
  `~/.local/share/knotq` on Linux, `KnotQ` under `LOCALAPPDATA` on Windows, and
  anything a `KNOTQ_DATA_DIR` points at. That is the user's workspace — schemes,
  daily pages, CRDT state, sync cursors. Most of it cannot be recreated. It is
  not a cache and it must never appear in a cleanup command.
- **`~/Library/Developer/Xcode/Archives`.** These carry the dSYMs for builds
  already in the App Store; without them a crash report from a shipped build
  cannot be symbolicated.
- **A target directory with a build or test running in it.** Deleting artifacts
  out from under `cargo` produces confusing linker failures and can fail a
  suite in a way that looks like a real regression.

## Procedure

1. **Check nothing is running first.** `pgrep -fl "cargo|wrangler|gradle|xcodebuild"`.
   A sync fuzz run takes the better part of an hour and holds `app/target` the
   whole time. If anything is live, stop here and come back.
2. **Measure before and after**, so the result is a number and not a feeling:
   `du -sh app/target app/mobile/target ~/Library/Developer/Xcode/DerivedData ~/Library/Developer/CoreSimulator/Devices`.
3. **Incremental caches first** — the cheapest, safest, largest single win.
   `rm -rf app/target/*/incremental app/mobile/target/*/incremental`.
   Costs one slower rebuild, risks nothing.
4. **Superseded cargo artifacts.** `cargo sweep` is the tool that understands
   which artifacts the current fingerprints still reference
   (`cargo install cargo-sweep` if missing), run from `app/` and again from
   `app/mobile/`:
   `cargo sweep --time 3` keeps anything touched in the last three days;
   `cargo sweep --installed` keeps only what the installed toolchains can use.
   Without it, `cargo clean -p <crate>` for the heaviest crates is the blunt
   alternative — `cargo clean` with no arguments throws away everything and
   costs a full cold rebuild of the workspace.
5. **Cross-compilation targets you are not currently building.** Under
   `app/mobile/target`, each of `aarch64-apple-ios-sim`, `x86_64-apple-ios`,
   `aarch64-linux-android` and `x86_64-linux-android` is its own ~1.7G tree.
   Delete the ones for a platform you are not about to build; they rebuild from
   source.
6. **Simulator devices.** `xcrun simctl delete unavailable` removes devices
   whose runtime is no longer installed — usually the bulk of the 24G. List
   runtimes with `xcrun simctl runtime list` before deleting any of them, and
   keep the versions the release process actually tests against.
7. **Xcode DerivedData** regenerates: `rm -rf ~/Library/Developer/Xcode/DerivedData/*`
   forces a full Xcode rebuild and nothing worse.
8. **Agent and worktree leftovers**, which are easy to forget:
   - Scratchpad target directories from agent sessions
     (`/private/tmp/claude-*/**/scratchpad/*target*`) — often several gigabytes
     each and dead the moment the session ends.
   - Stale git worktrees: `git worktree list`, then remove the directory and
     run `git worktree prune`. A worktree carries its own `target`.
9. **Re-measure and report the delta.**

## Rebuild cost

Everything above is regenerable, which is the point — the only cost of being
wrong is time. Budget a full cold workspace build at roughly two minutes for a
release profile and longer for debug plus the test binaries, and a full Xcode
rebuild on top if DerivedData went too. Do not do this immediately before a
release build or a deploy gate run.
