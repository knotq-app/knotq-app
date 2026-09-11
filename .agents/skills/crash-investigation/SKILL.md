---
name: crash-investigation
description: Investigate KnotQ desktop, iOS, or Android crashes and preserve enough symbol/build context to identify the source.
---

Use when a store dashboard, user report, simulator log, or production log shows
a crash.

- Start with exact app version/build, platform, OS version, device model,
  process, timestamp, and whether the report is symbolicated. Preserve the
  matching archive/dSYM or Android mapping file and verify UUID/version before
  reading frames.
- Treat `NO_CRASH_STACK`, aggregate counts, watchdogs, and framework-only frames
  as evidence of impact but not source attribution. Search the matching release
  source/history and reproduce with the same lifecycle/locale/timezone.
- iOS: inspect Organizer crash groups and device/simulator logs, then compare
  symbolicated and unsymbolicated reports. Android: inspect Play crash/ANR
  clusters, logcat, tombstones, and mapping files.
- Audit release-path `fatalError`, force unwraps, unchecked indexing, blocking
  UI work, detached callbacks, and stale UIKit/TextKit objects. Turn confirmed
  invariants into recoverable errors where that does not hide data loss.
- Add a deterministic regression test and a diagnostic log field for every
  confirmed trigger. Never claim a root cause from a framework frame alone.
