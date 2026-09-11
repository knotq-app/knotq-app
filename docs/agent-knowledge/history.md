# KnotQ failure history and lessons

This is a compact index of lessons recovered from the repository's commit and
test history. It is evidence for investigation, not a claim that unrelated
future failures have the same cause.

## Sync and persistence

- Empty/out-of-order scheme documents previously caused cursor/materialization
  wedges. The recovery work is concentrated around the shared sync engine and
  `pull_isolation`/`sync_wedge_regressions` tests. Compare content, pending edits,
  cursors, and CRDT bytes—not just final equality.
- Deferred Daily Queue documents were introduced for cold-start performance.
  Any change to pull, save, restart, or date navigation must check that deferred
  bytes remain durable, only the requested day hydrates, and a changed deferred
  document is later materialized from CRDT state rather than a stale plain file.
- Account switching and stale sockets are high-risk because a superficially
  successful pull can belong to the previous account. Exercise token/account
  identity, reseed obligations, logout, and fresh-device joins.

## Editor and UI

- A historical iOS TextKit crash occurred when autocorrection shortened text
  while embedded block display invalidation ran before layout consumed the edit.
  The production mitigation defers the display refresh; table deletion,
  marked-text, shrink-edit, and fast-typing tests protect it.
- Remote reloads must preserve semantic line identity and adapt UTF-16 caret
  position. Absolute offsets are invalid after edits above or before the caret.
- Repeated SwiftUI/UIKit mounting, first responder changes, keyboard animation,
  and stale async snapshot results are common UI-glitch surfaces even when unit
  tests pass.

## Performance lessons

- Historical work reduced keystroke cost by avoiding whole-buffer rebuilds,
  repainting only visible rows, batching CRDT reconciliation, and moving encode
  work off the UI thread. Benchmark changes against those invariants.
- A clean launch must not decode every deferred historical daily. A faster
  benchmark that loses or hides content is a correctness regression.
- Use fixed seeds, explicit timezone/device/build metadata, repeated samples,
  and existing performance budgets. Do not trust one local timing.

## Crash evidence

- Store dashboards may only expose aggregate counts. Xcode Organizer can show a
  framework-only or `NO_CRASH_STACK` group when symbols/diagnostics are absent.
  Preserve the exact archive, dSYM UUID, app version/build, OS, and device before
  drawing conclusions.
- Unsupported storyboard initializers using `fatalError` are intentional only
  if the construction path is impossible; every new force unwrap or unchecked
  index in a release path needs an explicit invariant or recoverable error.
