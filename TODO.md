# Known gaps

**Updated 2026-09-20.** These notes track confirmed data-loss/convergence
bugs and deferred release work. Current deploy-blocking status: 0a, 0b, 0c, 0d,
0e, 0f, 0g, 0h, 1, and 2 are fixed and verified; 3 and 5 remain backend/ops gaps, not
sync-convergence bugs. Item 4 remains explicitly deferred undo-history work.

## How this class of bug is found now: the projection law

Most of the entries below were reported the same way — "a field changed during
a sync and no other device wrote it" — and each took a long, seed-specific
investigation to attribute. They share one underlying shape, and it is worth
stating on its own because it needs no server and no second device:

> **A device's plain `Workspace` is exactly what its own CRDT documents
> materialize to.**

Two devices converge because Yrs merges their documents deterministically. That
guarantee only reaches the user if what the user sees *is* the document. Once a
plain workspace holds a value its own documents never did, the next landing
materializes the document's value instead — and to the user, and to the
no-silent-loss oracle, that is indistinguishable from a remote change nobody
made. Worse, the pre-pull repair (`queue_local_only_documents_before_pull`)
reads the difference as a local edit and re-asserts the stale value *every
sync*, which is where the "wedged: N unpushed edits after settling" failures
came from.

The law lives in `shared/sync/src/projection.rs` and is checked in three
places:

- `desktop/state/tests/projection_law.rs` — a randomized single-device harness.
  Hundreds of seeds in seconds, no server, no threads; a violation names the
  command that broke it.
- The production-path fuzzer checks it after **every** local step, sync landing
  and relaunch (`production_fuzz/device.rs`), so a violation names the step
  rather than the sync three steps later that surfaced it.
- `KNOTQ_CHECK_DISK=1` runs the same comparison against the *data directory*
  after each save, landing and shutdown. That is the only way to see a Daily
  page outside the loaded window: it is absent from `state.workspace`
  altogether, so the in-memory law cannot look at it.

When adding a repair, a normalization, or any path that writes one half of a
device's state, the question to answer is "does the other half get the same
write?" — 0d, 0e and 0f below were each a *no*.

## 0a. [FIXED] Workspace identity re-adoption never actually persisted, causing repeated re-keying that eventually dropped a scheme's binding

**Discovered 2026-09-17** via a 500-seed sweep of
`desktop_production_single_account_fuzz`'s exact scenario (1 account, 3
devices, no chaos, `maintenance_coverage: true`) against unmodified `main`
(commit `81bcd7f`) — **10 of 500 seeds failed**, a ~2% rate the default
`KNOTQ_FUZZ_SEEDS=6` never samples. One of them (seed 10404) was outright
scheme loss (`sync lost scheme ... "scheme 9781" that no device deleted`);
the rest were `ItemMeta`/`ItemContent`/`SchemeParent`/`FolderArchived`
fields reverting with no other device ever writing them (folded into 0b
below, which is now fixed).

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
far more instances of the 0b bug class than before, not introducing a
new one. Confirmed by: every new failure's violation type already appears
in 0b's catalog; the two mechanisms are structurally unrelated (0a is
workspace-identity/CRDT-document-binding, 0b is item-field materialization);
and this fix touches only workspace-index-level reconciliation, never item
fields, so it has no direct path to producing an `ItemMeta` divergence.

## 0b. [FIXED] Item-field edits and concurrent carryover can revert acknowledged values

**Resolution 2026-09-18:** the durable field-level journal now merges
successive acknowledged edits instead of replaying stale complete snapshots;
restart provenance preserves edits that cross a scheme or folder boundary;
restart guards prevent stale whole snapshots from being re-applied to an
unchanged scheme; and archived-scheme parent cleanup is modeled as index
normalization rather than data loss. The full residual set from the earlier
wide sweep (10307, 10313, 10316, 10320, 10370, 10379, 10408, 10418, 10421,
10439, 10441, 10452, 10454, 10455, 10469, 10477) now passes targeted replay.
The direct regression and the mandatory stress suite are also green.

**What got fixed:** scheme-level metadata (name/colour/gsync/source) reverting is gone from this sweep — `capture_local_scheme_edits`/`reassert_local_scheme_edits` (new in `moved_edits.rs`) closes that half. Item-field reverts for items that stayed in the same scheme (the originally-diagnosed gap, `landed_scheme == local_scheme` no longer short-circuiting) are also reduced.

The paragraphs below preserve the pre-fix investigation notes; they are not
current open failures. The residual seed list above now passes targeted replay.

**Historical pre-fix finding — now resolved:** the residual 16 seeds shared a
signature in which an `ItemMeta`/`ItemContent`/`ItemIndent`/`ItemPlacement`
field (or `SchemeParent`/`FolderExpanded`) silently reverted to a default or
blank value. The durable field-level journal and restart provenance fixes
cover this acknowledged-edit path; the residual seed list now passes targeted
replay and the mandatory 800×400 release gate.

**Original findings below, still relevant background** (from before this session's 0a/0c fixes; the specific seed-10307 trace is stale, since 0a's own RNG-stream shift changes which scenario a given seed number plays out — but the underlying carryover-race mechanism described is unverified either way, not re-confirmed after 0c):

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

**Suggested direction for the residual 16/500:** since capture/reassert
cannot protect an edit that is no longer pending, the fix has to be on the
*materialization* side — find what specific landing path can overwrite an
already-acknowledged field with a default value with no other device
involved (candidates: `replace_workspace_from_sync`/`_from_squash` in
`desktop/state/src/state.rs` rebuilding a scheme's materialized `Item` from
a CRDT document that is itself missing the field for some reason, or a
squash/compaction round-trip losing a field it shouldn't). Pick the
lowest-numbered failing seed (10307) and trace the CRDT document's own
content around the step where the checker reports the change, not just the
materialized `Workspace`, to see whether the field is missing from the CRDT
itself (a write that never landed) or present-but-ignored during
materialization (a read bug). For the carryover race described above, make
`daily_queue_carryover_command` (or its landing) resolve concurrent carries
of the same source item deterministically rather than racing — still
unverified whether it's a distinct mechanism from the revert-to-default
pattern above or the same one reached differently. As always: extend the
fuzzer with a dedicated scenario before considering this fixed, and re-run
the full 500-seed sweep (not just the default 6) after each change, since a
green default run has repeatedly not meant a green wide one for this bug.

## 0c. [FIXED] First-join workspace-index populations and scheme metadata could diverge during ordinary sync

**Root cause, confirmed by seed 4 and seed 10000 tracing:** a fresh install's
workspace-index CRDT was populated under its pre-account random `WorkspaceId`,
then merely re-keyed onto the account id before the first push. Another device's
canonical population consequently did not deduplicate with it, leaving Yjs with
different node membership/field winners on the server and the joining device.
The same pull-before-push landing could overwrite a local scheme rename or
recolour even though its operation was still pending; item edits already had a
capture/reassert path, but scheme metadata did not.

**Fix:** first-join sync now canonically re-populates the workspace index before
the first pull/push, then re-expresses the current local workspace as an edit on
that canonical base (`desktop/app/src/app/sync_service/snapshot.rs`). The store
keeps the corresponding canonical recovery path for edits racing first-sync
landing, and only re-expresses population bases whose plain scheme actually
changed (`desktop/state/src/store.rs`). Pending scheme metadata fields are now
captured and reasserted alongside item fields (`desktop/state/src/moved_edits.rs`).
A shared CRDT regression covers independent name and colour edits after shared
population; the production fuzzer retains the focused in-flight scenario.

**Verified:** pinned seed 10404, seed-4 replay, seeds 10000/10002, seeds 1/2/4,
the scheme-field regression, bug #1's in-flight-edit scenario, the relaunch
pending-edit scenario, and the full `cargo test -p knotq-app --bin knotq` run.
The full run and the wider release-mode property sweep are green; none of the
current failures involve first-join population divergence or scheme metadata
reverting.

## 0d. [FIXED] A marker family the line's marker cannot draw could never round-trip

**Found 2026-09-20** by the projection law, on-disk variant
(`KNOTQ_CHECK_DISK=1`, chaos fuzz seed 2): a Daily page's item held
`marker: Checkbox` with `marker_family: Rings` in the CRDT document and
`Standard` in the plain scheme file, permanently.

**Root cause:** the plain scheme file writes a line's marker and family as one
token (`Item::marker_token`, e.g. `bullet.rings`), and that token *drops* a
family `MarkerFamily::is_valid_for` rejects — a ring is a bullet glyph, a
checkbox cannot draw one. The CRDT document stores `marker_family` as a field
of its own and keeps whatever it is given. So the combination is
representable in one half of the data directory and not the other, and once an
item reached it the two halves disagreed for good. `SetItemMarkerFamily`
already validated the family, but `SetItemMarker` could change the *marker* out
from under a valid family.

**Fix:** `Item::enforce_marker_constraints` — already the central per-item
invariant, already called by insert/replace/set-marker and by
`Workspace::normalize_item_markers` — now resets a family its marker cannot
draw. The model therefore only ever holds values both halves can represent.
Regression: `changing_a_marker_drops_a_family_the_new_marker_cannot_draw`
(`desktop/commands/tests/item_cmds.rs`).

## 0e. [FIXED] A sync's marker repair reached the plain workspace but not the documents

**Found alongside 0d.** `sync_snapshot_in` runs three repairs on the pulled
workspace (identity, folder tree, item markers) and then queued CRDT updates
for them with `sync_changes(workspace, &ChangeSet::default().workspace())` —
**the workspace index only**. The identity and folder repairs are index-level,
so that was right for them; `normalize_item_markers` rewrites *item content*,
and its result never reached any scheme document.

**Fix:** `Workspace::normalize_item_markers` now returns the set of schemes it
repaired instead of a bool, and `queue_repair_crdt_updates` takes that set as
part of its change set. The signature change is the point: a caller can no
longer forget which documents a content repair has to be written to.

## 0f. [FIXED] `EnsureDailyQueue` could resurrect a row that had moved to another page

**Found by the projection law in the production fuzzer** (single-account seed
10004): a scheme showed one item that the CRDT placed in a different scheme,
from step 52 to the end of the run.

**Root cause:** a Daily page's placeholder row has a date-derived id, so two
devices creating the same day converge on one blank row instead of two. The
same determinism makes it re-mintable after the row has *moved* — a carry-over,
or an ordinary drag, takes the row (id and all) to another page and leaves the
day empty. Re-opening the day then re-created the id, so one item id was live
in two schemes at once. `dedupe_materialized_items` resolves that by keeping
the lowest scheme id and hiding the other, so the resurrected row was invisible
to every document-derived view while the plain workspace still showed it — and
the day silently emptied again on the next sync.

**Fix:** `ensure_daily_queue` leaves the day blank when its placeholder id is
alive in another scheme. Regression:
`a_day_whose_placeholder_moved_away_is_not_given_a_second_copy_of_it`
(`desktop/commands/tests/daily_cmds.rs`).

## 0g. [FIXED] Moving a folder across the archive boundary left its schemes half-trashed

**Found by `desktop/state/tests/projection_law.rs`, seed 4, in about a second.**
Moving a folder that sits *inside* an archived folder's subtree back into the
sidebar left its schemes in `recently_deleted` while the folder itself was
live. The CRDT workspace index enforces the opposite rule when it materializes
(a trashed scheme is kept out of the tree unless it is inside an archived
subtree), so the plain workspace no longer equalled its own document.

**Fix:** the rule is now stated once, as
`Workspace::archive_coherence_violations` / `reconcile_archive_membership`
(`shared/model/src/workspace/archive.rs`), and `move_node` calls the
reconciliation instead of every structural command reimplementing the rule.


## 0h. [FIXED] The reassert journal was a second, non-causal conflict resolver

**Symptom:** `still has 1 unpushed edit(s) after settling (wedged)` on the
single-account fuzz (seeds 10004, 10005), with the stuck edit being a
`ReplaceItem` the *landing itself* had authored, plus two devices diverging on
that item.

`desktop/state/src/moved_edits.rs` re-applies fields this device edited before
a sync landed. Its legitimate job is the **cross-document** case: each scheme
is its own CRDT document, so moving a line between schemes is a delete in the
source plus a fresh copy in the target, and an edit made to the source copy
lands on a line that no longer exists. Three scenarios prove that half is
load-bearing (turning the journal off fails
`a_line_retyped_while_another_device_moves_it_keeps_the_new_text` and two
others), so it stays.

Two things it was *also* doing had to go, because both make it a second
resolver racing Yrs:

1. Replaying the retained snapshot onto a line still in **its own** scheme.
   That conflict is already resolved causally; replaying manufactures a new
   local write on every landing. The `changed_after_bridge` escape hatch that
   re-armed it is gone: same-scheme is now an unconditional skip.
2. Keeping the journal entry alive **after** its cross-document bridge had
   been applied. A bridge is a one-shot repair: once this device has written
   its authored value into the destination document, that document owns it.
   Retained, two devices each re-assert their snapshot on every landing and a
   repair is authored per sync for ever — which is exactly the wedge. The
   entry is now retired as soon as its repair actually lands (a refused
   command keeps its entry so the repair is retried).

**Verified:** the whole `production_fuzz` suite — both fuzz configurations and
every pinned scenario — is green.


## 1. [FIXED] An edit made during a device's first-ever sync can be silently lost

**Repro (now a passing regression test, no longer `#[ignore]`d):** `cargo test
-p knotq-app --bin knotq
app::sync_service::production_fuzz::scenarios::an_edit_made_while_a_sync_is_in_flight_is_pushed`.

**Scenario:** device A signs in and fully syncs first, establishing an
account. Device B — a fresh install with its own starter content — signs into
the *same* account and starts its first sync. While that sync is still in
flight (server pull sent, not yet landed), the user recolors a scheme on B.
The recolor was silently dropped: A never saw it.

**Root cause:** the workspace-index CRDT document's very first population (for
either device) was authored under whatever `clientID` was live when it
happened, not a deterministic one derived from content. Scheme *content*
solved exactly this problem already (`YrsSchemeDocument::populate`, keyed by a
hash of the pre-edit content) — the workspace index never got the same
treatment. When two devices' independent from-scratch populations of the same
logical starter content collide, Yjs resolves every entry by clientID alone,
and whichever side has the higher one wins outright — including entries the
other side had already established correctly.

**Why it was harder than it looked:** the workspace-index document's identity
is volatile in a way scheme documents' isn't: `Workspace::new()` mints a fresh
random `WorkspaceId` per install, and the root folder id is *derived* from
it — so a device's content only becomes hashable-and-comparable with another
device's *after* it canonicalizes to the account's shared id (i.e. after its
first sync actually lands), and by design a device's local CRDT state gets
written well before that point, through at least three genuinely independent
places: `WorkspaceStore::new` (a fresh install's very first save),
`queue_workspace_bootstrap_updates` (`shared/sync/src/local_state.rs`, the
bootstrap push path), and `adopt_sync_workspace_identity` /
`reidentify_workspace_document` (`desktop/state/src/store.rs`,
`shared/sync/src/crdt/mod.rs`, where a device's local content gets re-keyed
onto the account's canonical identity once a pull actually lands). Two earlier
attempts (not in the tree) got as far as reaching the right code paths but
broke other convergence tests; see 0c below for the mechanism that finally
made this and those other tests pass together.

**Fix:** `WorkspaceCrdtDocuments::repopulate_workspace_canonically` (new,
`shared/sync/src/crdt/{mod.rs,workspace_index.rs}`) rebuilds a device's
workspace-index population under the account's canonical identity once it's
known — content-hash-deterministic, mirroring scheme population — and
re-applies any edit made on top of the old pre-canonical population as a
direct write on the fresh document (not a replayed byte diff, whose origin
pointers wouldn't resolve against the new doc's structs). `store.rs`'s
`adopt_sync_workspace_identity` drives this via a `workspace_population_base`
captured at construction / first edit, drops any stale pending push queued
under the old pre-canonical identity, and queues the repopulated document's
full state as the push (an incremental diff finds nothing new relative to the
freshly-built document, so a full snapshot is what actually carries the edit).
See 0c below for the further first-join canonicalization work layered on top
of this that made the wider regression suite pass too.

**Verified:** the pinned test above passes reliably (5+ repeated runs); full
`cargo test -p knotq-app --bin knotq` is green together with 0c's fix — see
0c's verification note.

## 2. [FIXED] A crash between saving the workspace and saving CRDT state loses pre-sync local edits

**Resolution 2026-09-18:** paired saves now write a durable pre-save
workspace base into `sync-state.json` before replacing plain workspace files.
The marker is cleared only after the pending queue and CRDT state are durable.
On relaunch, the store diffs the recovered plain workspace against that base
and re-expresses the changes as ordinary CRDT updates, including first-sync
offline edits to existing daily pages and newly created schemes.

**Verified:** `new_install_crashed_before_its_first_sync_joins_the_account`
is now a normal non-ignored production-path regression test, and the recovery
marker has a storage round-trip test.

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
