# Known gaps

**Deploy blockers as of 2026-09-17.** These are real, confirmed-reproducing
data-loss/convergence bugs, not hypothetical gaps. The mandatory sync-stress
gate (`./.github/scripts/run-sync-stress.sh --fuzz`, the 800×400 property
fuzz, `knotq-mobile-core`, the mobile WS integration test) is green *as
configured*, but item #0 exists precisely because that gate's default sample
size isn't wide enough to catch bugs at this rate — a green run is not proof
of no bugs here. Ordered by how much each matters.

## 0a. [FIXED] Workspace identity re-adoption never actually persisted, causing repeated re-keying that eventually dropped a scheme's binding

**Discovered 2026-09-17** via a 500-seed sweep of
`desktop_production_single_account_fuzz`'s exact scenario (1 account, 3
devices, no chaos, `maintenance_coverage: true`) against unmodified `main`
(commit `81bcd7f`) — **10 of 500 seeds failed**, a ~2% rate the default
`KNOTQ_FUZZ_SEEDS=6` never samples. One of them (seed 10404) was outright
scheme loss (`sync lost scheme ... "scheme 9781" that no device deleted`);
the rest were `ItemMeta`/`ItemContent`/`SchemeParent`/`FolderArchived`
fields reverting with no other device ever writing them (folded into 0b
below, which is still open).

**Root cause, confirmed for seed 10404** via targeted tracing (not just
reading): `merge_sync_crdt_states`
(`desktop/state/src/store.rs`) re-adopts the account's canonical workspace
identity whenever `sync_workspace.sync.id != self.workspace.sync.id`, via
`adopt_sync_workspace_identity`. That function re-keys the CRDT document's
*external* binding (`WorkspaceCrdtDocuments::reidentify_workspace_document`,
`shared/sync/src/crdt/mod.rs`) by copying the document's content verbatim
onto a freshly-keyed document — but the `sync` metadata *stored inside that
content* (the "meta" map) still names the **old** identity, because
`reidentify_workspace_document` only rebinds the document, it never rewrites
its own stored fields. `adopt_sync_workspace_identity` sets
`self.workspace` to the corrected copy in memory, but a few lines later in
the *same* `merge_sync_crdt_states` call, `apply_remote_updates`
re-materializes the workspace fresh from the CRDT document's content —
still holding the stale identity — and overwrites `self.workspace` right
back to it. The identity therefore never actually settles: `workspace.json`
keeps saving the stale id, so the mismatch is detected again on the very
next sync, triggering another re-key of an already-once-rekeyed document —
and on one of those later cycles the scheme's workspace-index binding was
dropped. (The exact reason the *second* re-key can lose content while the
first doesn't was not further isolated — the fix removes the repeated
re-keying entirely, which matters more than fully explaining its failure
mode.)

**Fix (committed):** `adopt_sync_workspace_identity` now reconciles the
corrected identity into the re-keyed document's own content immediately
(`self.defer_crdt(WorkspaceCrdtChangeSet::default().workspace());
self.flush_crdt();`), mirroring the existing `repair_workspace_index`
pattern in the same file, instead of relying on a future flush that never
arrives before the next materialization clobbers it. Once the identity
settles on the first successful adoption, later syncs see matching ids and
skip re-adoption entirely.

**Verified:** the pinned regression test
(`a_scheme_created_in_flight_survives_an_unrelated_replace_fallback`,
`desktop/app/src/app/sync_service/production_fuzz/mod.rs`, no longer
`#[ignore]`d) passes; full `cargo test -p knotq-app --bin knotq` is green
(210 passed, 0 failed, 3 ignored).

**Important side effect, understood and expected, not a new bug:** a
500-seed sweep *with the fix* shows **34 of 500 seeds failing** — up from
10. Seed 10404 is confirmed gone from the list; 6 of the original 10
(10304, 10348, 10404, 10475, 10477, 10484) are fixed; the other 4 (10307,
10313, 10379, 10455) still fail unchanged. All 30 "new" failures match the
*exact same* pre-existing signature shapes already cataloged in 0b below
(`ItemMeta`/`ItemContent`/`ItemIndent`/`SchemeParent`/`FolderArchived`
changing unilaterally) — none show a new category. The mechanism: before
this fix, *every* sync on *every* device hit the identity-mismatch-and-rekey
path (confirmed by tracing — the workspace identity never settled), each
time minting a document under a fresh random clientID
(`YrsJsonDocument::for_replica(new_id, kind, None)`). The fix makes identity
settle after the first successful adoption, so that repeated, per-sync
random-id consumption stops happening for the rest of the run. In the fuzz
harness's deterministic-id test mode (production uses true randomness, so
this class of effect cannot happen there), that substantially reshuffles
which scenario each seed number plays out from that point on — surfacing
far more instances of the still-open 0b bug than before, not introducing a
new one. Confirmed by: every new failure's violation type already appears
in 0b's catalog; the two mechanisms are structurally unrelated (0a is
workspace-identity/CRDT-document-binding, 0b is item-field materialization);
and this fix touches only workspace-index-level reconciliation, never item
fields, so it has no direct path to producing an `ItemMeta` divergence.

## 0b. [OPEN] Item-field edits can revert with no capture/reassert protection outside the narrow "moved to another scheme" case, plus a separate concurrent-carryover race

**Status update 2026-09-17: more frequently observed after 0a's fix (see above), not caused by it.** Now the dominant remaining failure class in a wide sweep (34/500 seeds, all but 4 are this bug).

`capture_local_item_edits`/`reassert_local_item_edits`
(`desktop/state/src/moved_edits.rs`) is narrower than it looks: it only
re-applies a captured field edit when the item *moved to a different
scheme* (`landed_scheme != local_scheme`). If the item stayed in the same
scheme and a sync's replace fallback simply reverted its field value in
place, the code explicitly `continue`s and does nothing — its own module
doc confirms the scope ("Keeping a line edit when another device moves the
line to another scheme"), a narrower, earlier-motivated problem than general
in-flight-edit protection. A field edit that doesn't involve a cross-scheme
move has **no** protection against a replace fallback at all.

**A separate mechanism, confirmed for one case (seed 10307):** two
*different* devices independently ran the Daily Queue carryover command
(`daily_queue_carryover_command`, `desktop/state/src/daily_queue.rs`) around
the same real time, each believing (from its own, not-yet-synced view) that
today's scheme was still blank. `daily_queue_carryover_command`'s
idempotency check (`existing.contains(&item.id)`) only guards against
*re-running carryover on a device that already saw it land* — it does
nothing for two devices racing to carry the same source item *concurrently*,
before either has seen the other's copy. There is already a passing
dedicated scenario for concurrent carryover
(`a_line_carried_over_by_two_devices_at_once_keeps_its_text_once` in
`scenarios.rs`), but it only asserts the item's *text* survives once, not
other `ItemMeta` fields (marker, in that case).

**Note:** that carryover-race trace was from *before* 0a's fix landed. Seed
10307 was re-checked *after* 0a's fix (its RNG-stream shift changes which
scenario a given seed number plays out, per 0a's own side-effect note above)
and now fails differently: `Item(b7502b85...) ItemContent` and
`Item(f36f46fa...) ItemMeta` both revert on device 0's sync at step 68,
correlated with a server compaction sweep at step 61 that ran just before
it. That timing correlation was investigated and **ruled out**: the shared
`knotq-sync` crate has its own dedicated compaction-convergence fuzz
(`compaction_sweep_fuzz_converges` / `run_seed_compaction`, exercising the
exact same `MemoryServer::run_compaction` v1→v2→v1 transcode desktop's fuzz
also uses), and a 300-seed sweep of it (`KNOTQ_FUZZ_SEEDS=300
KNOTQ_FUZZ_STEPS=160 cargo test -p knotq-sync --test sync_property_model
compaction_sweep_fuzz_converges --release`) found **zero** convergence
failures — the compaction mechanism itself is sound. Seed 10307's new
manifestation is therefore most likely the same "no protection outside
moved-scheme" reassert gap described above, reached via a different code
path than a genuine compaction defect; not confirmed further.

**Not root-caused to a single unified mechanism; likely two-plus distinct
gaps sharing the same oracle signature.** Needs its own investigation
session — and re-tracing any specific seed only after re-confirming its
current failure shape, since 0a's fix already changed at least one seed's
manifestation once.

**Suggested direction, not attempted:** for the general item-edit case,
extend `reassert_local_item_edits`'s protection beyond the "moved scheme"
case to any field edit made since the watermark, regardless of where the
item ended up. For the carryover race, make `daily_queue_carryover_command`
(or its landing) resolve concurrent carries of the same source item
deterministically rather than racing. As always: extend the fuzzer with a
dedicated scenario before considering either fixed, and run the **full**
`production_fuzz` suite after each step, not just the target seed.

## 1. An edit made during a device's first-ever sync can be silently lost

**Repro:** `cargo test -p knotq-app --bin knotq
app::sync_service::production_fuzz::scenarios::an_edit_made_while_a_sync_is_in_flight_is_pushed
-- --ignored --nocapture` (currently `#[ignore]`d in
`desktop/app/src/app/sync_service/production_fuzz/scenarios.rs`).

**Scenario:** device A signs in and fully syncs first, establishing an
account. Device B — a fresh install with its own starter content — signs into
the *same* account and starts its first sync. While that sync is still in
flight (server pull sent, not yet landed), the user recolors a scheme on B.
The recolor is silently dropped: A never sees it.

**Root cause:** the workspace-index CRDT document's very first population (for
either device) is authored under whatever `clientID` was live when it
happened, not a deterministic one derived from content. Scheme *content*
solved exactly this problem already (`YrsSchemeDocument::populate`, keyed by a
hash of the pre-edit content) — the workspace index never got the same
treatment. When two devices' independent from-scratch populations of the same
logical starter content collide, Yjs resolves every entry by clientID alone,
and whichever side has the higher one wins outright — including entries the
other side had already established correctly.

**Why it's harder than it looks:** the natural fix ("populate the index from
the pre-edit workspace under a deterministic client id, mirroring scheme
content") is the right *idea*, but the workspace-index document's identity is
also volatile in a way scheme documents' isn't: `Workspace::new()` mints a
fresh random `WorkspaceId` per install, and the root folder id is *derived*
from it — so a device's content only becomes hashable-and-comparable with
another device's *after* it canonicalizes to the account's shared id (i.e.
after its first sync actually lands), and by design a device's local CRDT
state gets written well before that point, through at least three genuinely
independent places:

1. `WorkspaceStore::new` — a fresh install's very first save (`initial_dirty`)
   flushes and populates the document before any sync attempt exists at all.
2. `queue_workspace_bootstrap_updates` (`shared/sync/src/local_state.rs`) —
   the bootstrap push path, for a device with nothing to merge.
3. `adopt_sync_workspace_identity` / `reidentify_workspace_document`
   (`desktop/state/src/store.rs`, `shared/sync/src/crdt/mod.rs`) — where a
   device's local content gets re-keyed onto the account's canonical identity
   once a pull actually lands.

An attempted fix that made all three of these populate deterministically
(content-hash clientID, mirroring scheme content, with the pre-edit base
threaded through as a new `workspace_population_base` and rebuilt at
reidentify time) got as far as reaching the right code paths with the right
data, but still didn't make the target test pass, **and — more importantly —
broke five previously-passing regression tests** (`a_folder_archived_...`,
`two_devices_changing_different_fields_of_one_scheme_keep_both`,
`two_devices_moving_and_renaming_one_scheme_keep_both`,
`two_devices_moving_folders_into_each_other_keep_both`,
`desktop_production_single_account_fuzz` — real scheme/folder/item loss in
scenarios that used to converge cleanly). That attempt was fully reverted
(nothing from it is in the tree); this note exists so the next attempt starts
from the diagnosis, not from zero.

**Suspected shape of a real fix:** probably needs a single, *unified*
call site for "populate the workspace-index document deterministically,"
reached from all three places above, with the deterministic-population
mechanism scoped to fire only *after* canonicalization (never on pre-sign-in
content) — and, critically, a test pass that runs the **existing** production
fuzz + regression suite (not just the target scenario) after every change,
given how easily this touches the other convergence tests. Extend
`shared/sync/tests/sync_property_model.rs` / the desktop `production_fuzz`
harness with a seed for this specific race before considering it fixed, per
this repo's rule about extending the fuzzer rather than hand-verifying one
scenario.

## 2. A crash between saving the workspace and saving CRDT state loses pre-sync local edits

**Repro:** `cargo test -p knotq-app --bin knotq
app::sync_service::production_fuzz::scenarios::new_install_crashed_before_its_first_sync_joins_the_account
-- --ignored --nocapture`.

**Scenario:** a brand-new install (never synced) crashes after its save task
wrote `workspace.json` but before it wrote the pending queue and CRDT state.
On relaunch, the CRDT is unseeded and nothing is queued, so the device's first
sync just adopts the account's existing index wholesale — the pre-crash local
edits (which only ever reached the plain workspace file) are gone.

**Not attempted this session** (deliberately, per user direction — this one
is rarer and needs real design work first). The test's own comment notes the
obvious fix (seed the CRDT from the plain files at launch) was already tried
and made things *worse* — it pushes an entire index lineage authored under
the pre-sign-in identity, which loses content elsewhere (the same
identity/clientID problem as #1, in fact — these two are probably the same
underlying disease). The real fix is described there as "re-root a pre-sign-in
lineage inside the CRDT index at first sign-in," which is exactly the kind of
mechanism #1 above also needs. **Worth investigating #1 and #2 together**,
since a real fix for the identity/population problem likely closes both.

## 3. No backup/DR for the backend's Durable Object sync state

A storage-corrupting bug on the Cloudflare side (SQLite-backed Durable Object)
would be unrecoverable data loss for anyone's synced content. No backup/export
route exists in `app/backend/cloudflare` today. Flagged as a launch blocker in
the 2026-07-12 production-readiness audit and explicitly deferred since — it's
a real project (enumerate every active workspace, a DO export route, a
fan-out/pagination architecture since Cloudflare's scheduled-handler execution
budget won't fit a naive daily sweep at scale, an R2 retention/expiry policy,
and it has to interact correctly with the account-deletion purge path so a
backup doesn't become a GDPR regression by outliving a deletion request), not
something to do blind.

## 4. Desktop undo doesn't survive a sync that touches the scheme you're mid-undo-history on

If a remote edit lands on scheme X while you still have local undo history for
X, that history is wiped rather than preserved per-scheme (other schemes'
undo history is already correctly scoped and survives). Explicitly deferred by
the user in the original 2026-06-26 undo-scoping rework; still deferred.

## 5. Cloudflare-side rate limiting is unverified in production

The backend's native rate-limit bindings aren't declared in `wrangler.toml`,
so `enforceRateLimit()` silently no-ops when unbound and the backend relies
entirely on zone-level WAF rules that live outside this repo. Whether those
are actually configured in prod has not been verified from this checkout.
