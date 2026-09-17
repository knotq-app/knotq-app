# Known gaps

Not deploy blockers as of 2026-09-17 — the mandatory sync-stress gate
(`./.github/scripts/run-sync-stress.sh --fuzz`, the 800×400 property fuzz,
`knotq-mobile-core`, the mobile WS integration test) is green. These are real,
confirmed-reproducing bugs or gaps, kept here so they don't get lost, roughly
ordered by how much they matter.

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
