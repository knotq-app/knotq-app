---
name: knotq-sync-release-gate
description: Run and interpret KnotQ's mandatory cross-device sync release gates before shipping desktop, backend, or mobile changes.
---

Use this skill whenever a change is about to ship or a sync reliability claim needs evidence.

Run from `app/` and record exit status and test summaries:

```sh
./.github/scripts/run-sync-stress.sh --fuzz
KNOTQ_FUZZ_SEEDS=800 KNOTQ_FUZZ_STEPS=400 cargo test -p knotq-sync --test sync_property_model --release
(cd mobile && cargo test -p knotq-mobile-core --features accounts)
```

For a deeper audit, also run the ignored `daily_queue_multiorigin_stress` property test explicitly and run the real-WebSocket mobile convergence scenario with `KNOTQ_SYNC_BACKEND_URL` set to the local test backend before teardown. Do not treat early-return skips as passing network coverage.

Read failures in the assertions and scenarios, especially preservation, pending-queue drainage, fresh-device convergence, account isolation, restart persistence, and daily-document races. A green convergence result is insufficient if every device lost the same content. Never tag or deploy while a required gate is red; report platform or network coverage that was not executed.
