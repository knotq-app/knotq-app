# Mobile operations and diagnostics

The mobile checkout is a separate Git root at `app/mobile/`. Run commands from
there and inspect both `app/` and `app/mobile` status before editing. Never use
simulator success as evidence of notification delivery on a sleeping device.

## iOS verification

```sh
cd app/mobile
ios_simulator_name="$(xcrun simctl list devices available | grep -m1 'iPhone' | sed -E 's/^[[:space:]]*([^()]*) \\(.*/\\1/' | sed 's/^[[:space:]]*//;s/[[:space:]]*$//')"
test -n "$ios_simulator_name"
for tz in UTC America/New_York America/Los_Angeles Pacific/Auckland Asia/Kolkata; do
  TZ="$tz" xcodebuild test \
    -project ios/KnotQMobile.xcodeproj -scheme KnotQMobile \
    -destination "platform=iOS Simulator,name=$ios_simulator_name" \
    -only-testing:KnotQMobileTests -parallel-testing-enabled NO
done
```

The timezone matrix protects date-only parsing, DST boundaries, and date-line
rollovers. `KnotQMobileTests` covers editor/TextKit, remote merges and cursor
adaptation, timeline geometry, notifications, lifecycle gates, auth policy, and
bounded control-plane request deadlines. The current local simulator run passed
165 tests with zero failures or skips; retain the `.xcresult` bundle when
diagnosing a release-specific issue.
The simulator may emit framework-only warnings such as duplicate accessibility
classes or `BGTaskSchedulerErrorDomain` code 1; record them separately from
XCTest failures and do not attribute them to app code without a symbolicated
stack.

All incoming ISO timestamps must be parsed through `MobileDate.parseDateTime` (or
an equivalent parser that accepts both whole-second and fractional-second ISO
8601 forms). Cloudflare/JavaScript timestamps commonly include milliseconds, and
Foundation's formatter rejects those when configured with only
`.withInternetDateTime`; notification scheduling and local-day bucketing are
therefore both covered by the fractional timestamp regression tests.

Small Swift control-plane requests all pass through `MobileHTTPResponseLimits`.
That helper caps response bytes and applies a 15-second maximum request timeout
while preserving shorter caller deadlines; do not call `URLSession.shared` for
auth, billing, or account metadata directly.

The Rust mobile snapshot captures one `Utc::now()` for event completion, upcoming,
and overdue derivation. Keep those sections on the same instant: sampling the
clock separately can make an occurrence cross a boundary between sections and
also adds avoidable work on every editor refresh.

## Real mobile transport verification

The ordinary mobile-core test command is intentionally offline. The real
WebSocket test skips unless `KNOTQ_SYNC_BACKEND_URL` is set, so use the checked-
in harness from the mobile checkout when validating the transport seam:

```sh
cd app/mobile
./.github/scripts/run-mobile-ws-integration.sh
```

The harness starts Wrangler with test-only bootstrap enabled, applies D1
migrations to isolated local state, probes `/readyz` and `/__test/bootstrap`,
runs `mobile_two_device_convergence_over_real_websocket`, and always tears the
Worker down. A plain `cargo test` passing is not evidence that this seam ran.

## High-risk paths

- `WebAuthenticationSessionCoordinator` uses a generation gate. Canceling or
  replacing a browser session must resume its checked continuation with
  `CancellationError`; callbacks from an older session must never complete a
  newer one.
- `MobileNotificationScheduler` serializes reconciliation and adds the desired
  set before removing stale pending requests. Scheduling, authorization, and
  badge errors are logged under the `com.enigmadux.knotq/notifications` OS-log
  category. A notification action must finish its core write and bounded sync
  before calling the OS completion handler.
- `AppModel+Sync.swift` owns one serialized sync flight. Auth refresh failures
  are terminal only for explicit backend refresh-token errors; bearer/network
  failures stay retryable and must not sign the user out.
- `AppModel` increments `SyncSessionGeneration` whenever a login session is
  installed or signed out. Every async refresh/status/sync response must check
  the captured generation before mutating model state, so a late response from
  the previous account cannot overwrite the current account.
- `AppModel` captures a `RefreshQuery` (date, week, and daily-history depth) for
  every bridge snapshot and mutation. If navigation changes that query before
  the result returns, publish only the notification reconciliation and let the
  queued latest read publish the snapshot; otherwise an old-day completion can
  visibly overwrite a newer selection.
- Long-lived polling, Google-sync, debounce, and UI-delay tasks must treat
  `Task.sleep` cancellation as a hard return. Do not swallow cancellation and
  execute the post-delay side effect: scene transitions and sign-out routinely
  cancel these tasks while their delay is in flight.

## Useful diagnostics

```sh
# Stream app-owned logs while exercising a simulator flow.
xcrun simctl spawn booted log stream --style compact \
  --predicate 'subsystem == "com.enigmadux.knotq"'

# Inspect a finished test archive and retain it with the build metadata.
xcrun xcresulttool get test-results summary --path /path/to/Test.xcresult

# Capture installed simulator crash reports.
xcrun simctl spawn booted ls /Library/Logs/CrashReporter
```

For a store crash, retain the exact app version/build, device and OS, archive,
dSYM UUID, Organizer group, and whether the stack is symbolicated. Aggregate
store counts or a framework-only `NO_CRASH_STACK` group identify impact but not
the source line.

## Android counterpart

Run JVM tests and lint first, then connected smoke tests with animations enabled
and disabled. Filter `logcat` by the KnotQ process id before interpreting skipped
frames; emulator/launcher messages are not app performance evidence. Exercise
the same timezone matrix used by iOS and mobile Rust. Store crash diagnostics
with the Play version code, mapping file, device/OS, and the complete tombstone.

Android auth, subscription, and refresh responses go through the shared
`readUtf8Capped` helper (`MAX_HTTP_RESPONSE_BYTES` = 1 MiB). If a new control-plane
endpoint reads an `HttpURLConnection` stream directly, route it through that
helper and add an exact-cap/overflow test; never use unbounded `readText()` for
data received from the sync service. `isSecureSyncApiBase` and `httpJson` also
fail closed for non-HTTPS endpoints, URL userinfo, query, or fragment routing;
only loopback HTTP is permitted for local integration workers. Keep the iOS
`AppModel.isSecureSyncApiBase` policy in lockstep when changing this boundary.
