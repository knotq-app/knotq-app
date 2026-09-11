# Verification ladder

Run the smallest relevant layer first, then broaden. Record pass, fail, skip,
and why; do not collapse early-return transport tests into a green claim.

The reusable [test-ladder skill](../../.agents/skills/test-ladder/SKILL.md)
contains the copyable commands and platform-specific routing below. Keep this
document as the short reference for what evidence to report.

## Fast local checks

```sh
# app/
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p knotq-mcp --tests --release
cargo test -p knotq-sync --tests --release

# app/mobile/
cargo fmt --all -- --check
cargo clippy -p knotq-mobile-core --features accounts --all-targets -- -D warnings
cargo test -p knotq-mobile-core --features accounts
ios_simulator_name="$(xcrun simctl list devices available | grep -m1 'iPhone' | sed -E 's/^[[:space:]]*([^()]*) \\(.*/\\1/' | sed 's/^[[:space:]]*//;s/[[:space:]]*$//')"
xcodebuild test -project ios/KnotQMobile.xcodeproj -scheme KnotQMobile \
  -destination "platform=iOS Simulator,name=$ios_simulator_name"
./android/gradlew -p android :app:testDebugUnitTest :app:lintDebug

# app/backend/cloudflare/
pnpm run typecheck
pnpm run test:monitoring
pnpm test -- --reporter=dot
pnpm exec vitest run test/schema-hot-paths.test.ts --no-file-parallelism --maxWorkers=1
```

Static security analysis is run by the repository-local CodeQL workflows for
desktop Rust/Actions, backend TypeScript/Actions, and mobile Swift/Kotlin/
Actions. Those workflows are scheduled and also run on changes; they do not
publish application or backend artifacts.

## Reliability gates

- Shared property model: `KNOTQ_FUZZ_SEEDS` and `KNOTQ_FUZZ_STEPS`.
- Explicit Daily Queue multi-origin stress:
  `--ignored daily_queue_multiorigin_stress`.
- Mobile account/lifecycle suite: `cargo test -p knotq-mobile-core --features accounts`.
- Real backend HTTP/WebSocket suite:
  `./.github/scripts/run-sync-stress.sh --fuzz` from `app/`, requiring
  `backend/cloudflare` and its installed dependencies.
- Backend D1 migrations are applied by the Workers test setup; when adding one,
  run the auth-store/health tests and then the full backend suite.
- MCP real wiring:
  `desktop/mcp/tests/manual_e2e.sh target/debug/knotq` with throwaway data only.

## Release boundary

The sync stress gate is mandatory before a release tag, backend deploy, or
mobile store submission. Build and test artifacts locally as much as possible,
but stop before publishing unless the user explicitly authorizes it.

## Reporting format

End every substantial verification pass with:

1. exact commands and counts;
2. failures and whether they are production, test-fixture, or environment;
3. skipped transport/platform coverage and the missing prerequisite;
4. changed paths and residual risk;
5. artifact/archive/log locations when diagnosing a release.
