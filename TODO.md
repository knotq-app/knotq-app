# Known gaps

**Deploy blockers as of 2026-09-17.** These are real, confirmed-reproducing
data-loss/convergence bugs, not hypothetical gaps. The mandatory sync-stress
gate (`./.github/scripts/run-sync-stress.sh --fuzz`, the 800×400 property
fuzz, `knotq-mobile-core`, the mobile WS integration test) is green *as
configured*, but item #0 exists precisely because that gate's default sample
size isn't wide enough to catch it — a green run is not proof of no bugs
here. Ordered by how much each matters; #0 is worst.

## 0. The default fuzz corpus (6 seeds) is not wide enough — a ~2% per-seed convergence bug in ordinary, non-chaotic multi-device sync went undetected

**Discovered 2026-09-17** while investigating #1 below: a debug session ran
`desktop_production_single_account_fuzz`'s exact scenario (1 account, 3
devices, no chaos, `maintenance_coverage: true`) across a **500-seed sweep**
(`KNOTQ_FUZZ_SEEDS=500 KNOTQ_FUZZ_WORKERS=8 cargo test -p knotq-app --bin
knotq desktop_production_single_account_fuzz --release -- --nocapture`,
starting at seed 10,000 — so seeds 10000..10500) against the **unmodified,
already-shipped `main`** (commit `81bcd7f`, nothing from the #1 investigation
applied). **10 of 500 seeds failed** — a ~2% rate, in the *plainest* fuzz
configuration this repo has (no crashes, no dropped connections, no account
switching). The default `KNOTQ_FUZZ_SEEDS` is 6, which is why CI has never
hit this. Confirmed not a `--release`-only artifact: seed 10404 reproduces
identically in a debug build.

Failing seeds and their oracle violations (all "changed/lost with no other
device ever writing it" — i.e. the CRDT merge itself diverged, not a fuzz
harness bug):

- 10304: `Folder(...) FolderArchived` — `true -> false`
- 10307, 10475, 10484: `Item(...) ItemMeta` changed unilaterally
- 10313: `Item(...) ItemMeta` changed unilaterally
- 10348: `Scheme(...) SchemeParent` — `Some(folder) -> None`
- 10379: `Item(...) ItemMeta` changed unilaterally
- **10404: `sync lost scheme ... "scheme 9781" that no device deleted`** —
  outright scheme loss, root-caused below
- 10455: `Item(...) ItemMeta` changed unilaterally
- 10477: `Item(...) ItemContent` changed unilaterally

**Root cause, confirmed for seed 10404** (repro: add a temporary
`run_seed(10_404, Config { accounts: 1, initial_devices: 3, max_devices: 4,
chaos: false, maintenance_coverage: true, .. })` call and run with
`KNOTQ_FUZZ_TRACE=1 --nocapture`):

Device 1 creates a new scheme as an *in-flight edit* — during its own
`sync()`, after `run_sync` has snapshotted state but before `land_sync`
lands the result (`desktop/app/src/app/sync_service/production_fuzz/mod.rs`'s
`World::sync`, `in_flight_edits` loop). Landing goes through
`adopt_sync_workspace` (`desktop/app/src/app/sync_service/landing.rs:65`),
which tries `state.merge_workspace_from_sync` first and only falls back to
`state.replace_workspace_from_sync` (a **blind overwrite** of
`self.workspace`) when the merge isn't possible.

`merge_sync_crdt_states` (`desktop/state/src/store.rs:720`, guard at
`:749-757`) refuses the merge — returning `false`, forcing the replace
fallback — whenever the *pulled* workspace references **any** scheme binding
this device doesn't have a CRDT document for yet:

```rust
let known_documents = self.crdt.known_document_ids();
if sync_workspace.scheme_sync.iter().any(|(scheme, meta)| {
    meta.kind == SyncDocumentKind::Scheme
        && self.workspace.schemes.contains_key(scheme)
        && !known_documents.contains(&meta.id)
        && crdt_states.contains_key(&meta.id)
}) {
    return false;
}
```

This guard exists for a real reason (comment above it, and its own regression
test at "seed 10004": without it, a scheme re-created on another device stays
stale forever on this one). But it is scheme-agnostic — at step 13 in the
10404 trace, device 1's run applied 5 *unrelated* remote updates from other
devices, one of which was a binding for some other scheme this device hasn't
built a document for. That alone is enough to bail the merge **entirely**,
and the replace fallback then discards *every* local change made since the
watermark that isn't specifically protected — which is only item-field edits
(`capture_local_item_edits` / `reassert_local_item_edits`, called
unconditionally after `adopt_sync_workspace` in
`desktop/app/src/app/sync_service/production_fuzz/device.rs:326`'s
`land_sync`, and presumably the production `sync_service` task's equivalent).
A **newly created scheme** has no equivalent capture/reassert step, so it is
silently dropped from the workspace the moment *any* unrelated concurrent
scheme binding forces the fallback. `SchemeParent`/`FolderArchived` look like
the same gap for structural folder/scheme edits.

**Correction to the "protected" claim above:** re-reading
`capture_local_item_edits`/`reassert_local_item_edits`
(`desktop/state/src/moved_edits.rs`) — this mechanism is narrower than it
looks. `reassert_local_item_edits` only re-applies a captured field edit when
the item *moved to a different scheme* (`landed_scheme != local_scheme`); if
the item stayed in the same scheme and the replace fallback simply reverted
its field value, the code explicitly `continue`s and does nothing. Its own
module doc confirms the scope: "Keeping a line edit when another device moves
the line to another scheme," a narrower, earlier-motivated problem. So a
field edit that does NOT involve a cross-scheme move has **no** protection
against the replace fallback at all — this may independently explain some of
the `ItemMeta` violations, on top of whatever #0's scheme-loss mechanism
explains.

**But at least one `ItemMeta` violation (seed 10307) traced to something
else entirely — likely unrelated to the replace-fallback mechanism above.**
Item `99c6c352...`'s marker reverts `checkbox -> blank` with no cross-device
write. Tracing it: two *different* devices (2 and 0, steps 108 and 119) each
independently ran the Daily Queue carryover command
(`daily_queue_carryover_command`, `desktop/state/src/daily_queue.rs`) around
the same real time, each believing (from its own, not-yet-synced view) that
today's scheme was still blank. `daily_queue_carryover_command`'s
idempotency check (`existing.contains(&item.id)`) only guards against
*re-running carryover on a device that already saw it land* — it does
nothing for two devices racing to carry the same source item *concurrently*,
before either has seen the other's copy. There is already a dedicated
scenario for concurrent carryover
(`a_line_carried_over_by_two_devices_at_once_keeps_its_text_once` in
`scenarios.rs`) — passing — but it apparently only asserts on the item's
*text* surviving once, not its other `ItemMeta` fields (marker, in this
case). **Not root-caused further; likely a separate bug from #0's scheme
loss, in the carryover/concurrent-creation path rather than the
merge-vs-replace landing decision.** Needs its own investigation session —
don't assume the fix for #0 above also fixes this.

**Suggested direction for the scheme-loss mechanism specifically, not
attempted:** mirror the existing item-edit capture/reassert pattern for
whole-scheme and whole-folder creation — capture "locally known but not in
the incoming `sync_workspace`, created after the watermark" schemes/folders
before `adopt_sync_workspace` runs, and re-insert them after, regardless of
which path (merge or replace) was taken. Note this needs more than
re-inserting into `state.workspace.schemes`/`.folders`: the CRDT layer
(`self.crdt`) also has to know about the re-inserted entity, or the very next
sync's workspace-to-CRDT reconciliation will just delete it again — this is
exactly the kind of subtlety that makes this riskier than it first looks.
Keep the existing scheme-binding guard as is (it protects a different,
already-fixed bug) — the fix is what happens *after* the fallback fires, not
preventing it from firing. As always: extend the fuzzer (this exact scenario
is now a known, minimal repro,
`a_scheme_created_in_flight_survives_an_unrelated_replace_fallback` in
`desktop/app/src/app/sync_service/production_fuzz/mod.rs`, `#[ignore]`d) and
run the **full** `production_fuzz` suite after each
step, not just the target seed.

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
