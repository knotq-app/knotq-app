---
name: test-ladder
description: Run and report the correct KnotQ validation ladder across Rust, iOS, Android, MCP, and the backend.
---

Use when validating a change, investigating a regression, or preparing a
release candidate. Start narrow and broaden only after the nearest layer is
green; preserve unrelated dirty work in both the `app/` and `app/mobile/`
repositories and in `app/backend/` when present.

## Fast local ladder

From `app/`:

```sh
cargo fmt --all -- --check
cargo test -p knotq-sync --tests --release
cargo test -p knotq-mcp --tests --release
```

From `app/mobile/`:

```sh
cargo fmt --all -- --check
cargo test -p knotq-mobile-core --features accounts --release --lib -- --nocapture
./android/gradlew -p android :app:testDebugUnitTest :app:lintDebug --no-daemon --console=plain
```

On macOS, choose an installed iPhone simulator rather than hard-coding a
device name, then run the complete native test target:

```sh
simulator_name="$(xcrun simctl list devices available | grep -m1 'iPhone' | sed -E 's/^[[:space:]]*([^()]*) \\(.*/\\1/' | sed 's/^[[:space:]]*//;s/[[:space:]]*$//')"
test -n "$simulator_name"
xcodebuild test -project ios/KnotQMobile.xcodeproj -scheme KnotQMobile \
  -destination "platform=iOS Simulator,name=$simulator_name" \
  -only-testing:KnotQMobileTests -parallel-testing-enabled NO
```

From `app/backend/cloudflare/`:

```sh
pnpm run validate:deployment-config
pnpm run typecheck
pnpm test -- --no-file-parallelism --maxWorkers=1 --reporter=dot
pnpm audit --prod --audit-level=moderate
```

## Reliability and integration ladder

- Run `KNOTQ_FUZZ_SEEDS=800 KNOTQ_FUZZ_STEPS=400 cargo test -p knotq-sync
  --test sync_property_model --release` for deep shared convergence.
- Run the ignored `daily_queue_multiorigin_stress` explicitly when touching
  deferred Daily Queue, indexing, external-source, or persistence behavior.
- Run `./.github/scripts/run-sync-stress.sh --fuzz` from `app/` when the local
  backend checkout and dependencies are available. This is the real HTTP plus
  WebSocket transport gate; a missing backend or early-return test is a skip,
  not a pass.
- Run MCP's loopback E2E script with throwaway data after changing MCP bridge
  or sync wiring; unit tests alone do not prove the process boundary.

Before any desktop tag, backend deploy, or mobile store submission, the real
sync stress gate and the deep property-model command are mandatory. Testing
does not authorize publication. Report exact exit codes, test counts, skipped
transport prerequisites, and artifact/log paths rather than saying simply
“tests passed.”
