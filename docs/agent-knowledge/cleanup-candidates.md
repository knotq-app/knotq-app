# Cleanup candidates

This is a map for incremental refactors. It is deliberately not a request to
split files mechanically: the listed modules often contain state-machine or
FFI boundaries where an ill-timed extraction can change behavior.

## Highest-value candidates

| Area | Current shape | Safe first seam | Guardrail |
| --- | --- | --- | --- |
| `mobile/core/src/tests.rs` | Core sync/account tests remain here; archive, notifications, calendar imports, and deferred Daily Queue stress now live in `tests_archive.rs`, `tests_notifications.rs`, `tests_calendar.rs`, and `tests_daily.rs` | Move the remaining small search/editor seam only if it has a clear shared fixture boundary | Keep cross-module fixtures explicitly `pub(crate)` and run the full accounts suite after each move |
| `mobile/core/src/mobile_core_inner_ops.rs` | FFI operations plus persistence/sync orchestration | Extract pure request validation and command-to-update helpers first | Preserve UniFFI error strings and serialized request shapes |
| `mobile/ios/KnotQMobile/SchemeEditorChrome.swift` | Editor lifecycle and remote reload state machine; navigation/chrome now lives in `SchemeEditorNavigation.swift` | Extract only additional pure editor policy helpers when a focused test exists | Keep TextKit mutations on the main actor and retain integration tests |
| `mobile/ios/KnotQMobile/SchemeEditorTextView.swift` | TextKit delegate, marker concealment, input handling, and layout invalidation | Separate marker/display mapping from delegate event routing | Run the full iOS suite; protect against stale glyph-map and UTF-16 range crashes |
| `desktop/app/src/app/sync_service/snapshot.rs` | Background snapshot, pull/push, repair, persistence, and media coordination | Extract diagnostics and persistence decision helpers before moving orchestration | Preserve save ordering and account-switch re-identification semantics |
| `desktop/app/src/app/google_oauth/network.rs` | OAuth transport, refresh, calendar fetch, and response decoding | Extract response decoding and retry classification as pure functions | Test token redaction, clock skew, pagination, and non-US time zones |

## Refactoring rules

1. Make one seam per change and keep behavior-preserving refactors separate
   from fixes.
2. Add a focused test before extracting code when the behavior is subtle.
3. Prefer pure helpers around CRDT/index/date calculations; leave platform
   lifecycle and FFI ownership at the edge.
4. Measure before and after on the same fixture. A smaller file is not a
   performance improvement by itself.
5. Do not split generated UniFFI output or platform files merely to meet a
   line-count target.

## Current runtime signal

The desktop runtime smoke showed repeated orphan-document messages during
startup. These are now aggregated with a bounded sample, while non-benign
materialization gaps retain per-document detail. If the aggregate remains
large, investigate the workspace-index/content ordering in the shared sync
engine rather than suppressing the signal further.
