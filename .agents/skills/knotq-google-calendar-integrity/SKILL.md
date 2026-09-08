---
name: knotq-google-calendar-integrity
description: Audit and harden KnotQ Google Calendar import, recurrence timezone handling, deletions, and source reconciliation.
---

Use this skill for Google Calendar sync bugs, off-by-one-day reports, stale imported events, or integrity recovery.

Trace OAuth/listing, event pagination and sync-token paths in `desktop/app/src/app/google_oauth`, then recurrence expansion in `desktop/rrule`. Calendar recurrence weekdays and wall-clock times are local-calendar semantics; never reinterpret a UTC-converted timestamp as the source weekday. Test both a UTC-negative timezone and a UTC-positive zone such as `Asia/Taipei`, plus a DST boundary.

Calendar-list reconciliation must include deleted and hidden entries when importing, preserve picker filtering separately, and archive stale imported sources recoverably rather than silently dropping user-visible data. Event deletions, updates, recurring exceptions, missing calendars, duplicate sources, incremental sync, and full-sync fallback need unit coverage. When changing persisted formats, follow `desktop/storage-json/src/upgrade` and its fixture/rollback rules.

Report whether evidence covers live Google API behavior or only mocked/unit paths; do not claim production account integrity from local tests alone.
