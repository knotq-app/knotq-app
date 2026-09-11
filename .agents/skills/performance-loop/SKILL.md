---
name: performance-loop
description: Measure and improve KnotQ launch, editing, indexing, sync, and rendering performance with reproducible evidence.
---

Use for performance work or regressions. Make one bounded change at a time and
keep behavior and persistence formats unchanged.

1. Establish the scenario and baseline: cold launch, first frame, 2k/10k-line
   editing, daily history navigation, search/calendar rebuild, sync pull, and
   save. Record OS/device, build profile, timezone, seed, and sample count.
2. Prefer existing probes and budgets (`perf_budget`, edit/load-cost probes,
   core timing, sync timing) over ad-hoc sleeps. Repeat noisy UI timings and
   use ratios/shapes when absolute timing is machine-dependent.
3. Treat the UI thread as a scarce resource: snapshot/CRDT encoding, disk I/O,
   image decoding, and index rebuilds must not accidentally move onto it.
   Check cancellation and stale-result gates when async work completes.
4. For mobile, test low-power-ish lifecycle boundaries: background/foreground,
   repeated navigation, keyboard editing, rotation/size changes, and a slow or
   failed sync. For dates, run at UTC, DST transitions, negative/positive
   offsets, and a non-US locale.
5. Keep a short before/after note with the command, result, and residual cost.
   A faster benchmark that drops edits, skips deferred documents, or hides a
   stale UI result is a regression, not an optimization.
