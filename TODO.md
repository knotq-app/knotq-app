# Known gaps

**Updated 2026-09-24.** These notes track confirmed data-loss/convergence
bugs and deferred release work. Current deploy-blocking status: 0a, 0b, 0c, 0d,
0e, 0f, 0g, 0h, 0j, 0k, 0l, 0m, 0n, 0o, 0p, 0q, 0t, 0u, 0v, 1, and 2 are fixed
and verified; 0i (the account-switch exclusion) and 0w (the deep gate's 7 Daily-Queue seeds) are open, and 0r (one scheme colour) now passes but is untraced; 3 and 5 remain backend/ops gaps,
not sync-convergence bugs. Item 4 remains explicitly deferred undo-history work.

**Where the release-depth gate stands (300 seeds x 200 steps per
configuration).** Chaos 140 (0t), single-account 10290 (0u) and chaos 289 (0v)
are fixed; 220 and 287 — the account-switch deletion — were fixed before. See
"The parallel sweep can miss a failing seed" below.

> **Corrected 2026-09-24.** The sentence that stood here — "both configurations
> pass every seed, by sweep and by one-at-a-time replay (2026-09-23)" — was
> wrong. Single-account seed **10175** fails at 200 steps, inside the swept
> range, and `sync-fuzz.yml` has meanwhile been asking for a depth nobody could
> see failing because the job could not link. **0w** has the measurements: 7
> failing seeds at that file's 400 x 300, plus 10175 at 300 x 200, none of them
> a regression from v0.57.0.

10290 is not new. It fails identically on `472edf1`, measured at the same depth
in a clean worktree; the claim on that commit that single-account was "green
across all 300" was wrong. Verifying a gate claim against the actual baseline,
rather than against the last note about it, is worth the four minutes it costs.

> The release gate is the one to keep green. `release.yml` gates every build
> job on `needs: [sync-stress, mobile-accounts]`; it is not to be skipped,
> narrowed or marked `continue-on-error` to get a build out.

Two findings worth not re-deriving: a Daily page bound in the index with no
`nodes` entry is normal rather than corruption (clients rebuild it through
`ensure_daily_queue`, so do not rebuild it in `materialize_workspace_inner` —
that breaks the projection law from the other side), and a carryover's
displaced item id is derived from `(row, source date)`, so two devices rolling
the same day mint the same id and the line goes live in two documents.

**Depth matters, and the gate at depth was already red.** The PR gate runs the
production fuzzer at its default depth; at CI depth
(`KNOTQ_FUZZ_SEEDS=128 KNOTQ_FUZZ_STEPS=200`) commit `4abec2a` — before any of
0d–0l — fails **3 of 30** tests, on 50 of the 128 single-account seeds (10001,
10003, 10005, 10006, 10013, 10019, 10022, 10025, 10028–10032, 10034, 10036,
10038, 10039, 10043, 10045–10047, 10050, 10059, 10063, 10069, 10081, 10082,
10084–10088, 10091–10097, 10102, 10109, 10110, 10112, 10113, 10116, 10117,
10119, 10120, 10123, 10127), on chaos seeds 12 and 112, and on the *pinned*
regression seed 10404 once it is run at 200 steps instead of its usual 120. So
a failure at that depth is not a regression from this work; it is the backlog
this work is draining. Raising the gate's depth is worth doing only once the
sweep is green.

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


## 0i. [ROOT CAUSE FOUND] Switching accounts can delete the *other* account's data

**Verified 2026-09-21, and it is worse than this entry previously described.**
The earlier text assumed the loss was a projection-law problem on the switching
device and stated that "a plain Yjs merge cannot remove an entry the pusher
never saw". That premise is wrong, and so was the search it directed.

### The mechanism, verified in the code

Three facts compose into data loss:

1. **The same document id exists in every account.** A Daily page's document id
   is a hash of its *date* alone — `daily_queue_ids` in
   `shared/model/src/daily_queue.rs` takes only `date.to_string()` — and a
   scheme's is derived from its scheme id. No account is in the hash. Starter
   schemes have fixed ids, so every account holds `…101/102/103` too.

2. **The contents alias, not just the ids.**
   `stable_scheme_population_client_id` (`shared/sync/src/crdt/encoding.rs`)
   hashes `(document id, content)` — again no account. Two accounts that each
   hold "Daily 2026-09-14" with the same starter rows therefore hold
   *byte-identical Yjs structs*: same clientIDs, same clocks.

3. **A re-seed pushes the entire delete set.** `full_snapshot_updates` emits
   `encode_state_v1`, and `shared/sync/src/crdt/update_capture.rs` says so in
   its own words: "`encode_diff_v1` attaches the document's **entire** delete
   set to every delta … full state is emitted deliberately, via `force`/reseed
   paths".

So when a device switches accounts, it loads the source account's bytes under
ids the destination also uses, merges the destination's history into them, and
`queue_account_switch_reseed` pushes the union — **including tombstones the
device authored on the account it just left**. Those tombstones land on the
destination's identical structs and delete rows that account's other devices
still have.

Caught by chaos seeds 281 and 287 as "server state lost item … that no device
deleted". The fuzz *under-reports* it: the oracle's `destroyed_items` is global
across accounts, so a plain delete on the source excuses the loss on the
destination, and only carried-over daily rows are flagged. The real blast radius
is every fixed-id starter line and every daily row shared by date, both
directions. Mobile takes the same path.

Two corollaries: `adopt_sync_workspace_identity`'s disjointness guard can never
fire (every account knows the fixed-id schemes), and the store persists the
merged two-account history afterwards, so a later diff-fallback push re-sends
the foreign delete set even if the re-seed is fixed.

### A fix attempt that failed — do not repeat it as-is

Dropping the source account's scheme states before `from_states` and removing
`queue_account_switch_reseed` took the gate from **7 failing seeds to over
150**. Content has to follow the user across a switch; the heal path only
populates schema-less documents, so starting empty loses everything local that
the destination does not already have. Any fix must keep the *content* and drop
only the *history*.

### A second fix attempt that failed — and it rules out the obvious shape

This entry used to propose re-seeding with a history-free population rather
than `encode_state_v1`, on the grounds that it carries the content without the
tombstones. **That was tried on 2026-09-22 and took the gate from 3 failing
chaos seeds to 87**, plus 5 in the single-account configuration that had been
green.

The reason is already written down elsewhere in the crate, in
`adopt_squashed_document`: a rebuilt document "shares no Yjs history with its
predecessor, so a merge would double content" — which is why a squash is
*adopted*, replacing the local document, and never merged. The account-switch
re-seed is an ordinary push, so the server merges it. A history-free rebuild
pushed into a merge is therefore the one thing it must never be.

So dropping the history requires the destination to REPLACE rather than merge,
which means going through the epoch/squash mechanism rather than the pending
queue — a much larger change than this entry previously implied. The remaining
candidates:

1. Re-seed through an epoch bump, so the destination adopts instead of merging.
2. Put the account's workspace id into the document-id hash, so the two
   accounts cannot address the same document at all. A format change: it needs
   `storage-json/src/upgrade/`, a captured release fixture, and desktop and
   mobile shipped together. Note this is *not* needed to stop a server-side
   collision — `WORKSPACE_OBJECTS.idFromName(workspaceId)` already scopes every
   document per account — it is only about making the structs stop aliasing.
3. Scope the population clientID by account, which stops the structs aliasing
   without moving any document. Fleet-visible via
   `SCHEME_POPULATION_ENCODING_VERSION`, and the determinism is load-bearing
   *within* an account, so first-population dedupe has to keep working. The principled alternative is to put the account's workspace
id into the document-id hash so the two accounts can never address the same
document — a format change needing `storage-json/src/upgrade/`, a captured
fixture, and desktop+mobile shipped together.


## 0s. [FIXED] A device's first successful sync dropped what it edited before it

**Fixed 2026-09-22.** Production fuzz chaos seed 253: device 0 inserted a line
at step 19, every sync attempt until step 148 failed, and that first successful
sync lost the line. Device 0 never switches accounts and the lost id is a
random v4 — not one of the derived ids that alias across accounts — so this was
never 0i wearing a different hat, even though the seed runs in the two-account
configuration.

The cause was the `document_cursors.is_empty()` guard in
`queue_local_only_documents_before_pull`. A device that has never synced with
this server skipped the whole pre-pull repair, so content only it held was
never written into the documents before the pull merged the server's copy over
them. The post-pull bootstrap could not recover it either: it only repopulates
documents that are still schema-less, and by then the pull has populated them.

The guard had two real reasons behind it, and only one of them is about the
index. Writing this device's workspace index before the account's is pulled
costs the account everything (`offline_device_join.rs`). But most of a
never-synced device's plain *content* is not its own either — a fresh install's
starter lines are the same lines the account may have deleted long ago, and
re-asserting them resurrects them. Letting the whole content half run took the
gate from 5 failing seeds to **17** for exactly that reason.

Both concerns are satisfied at once by asking which lines the device can prove
it authored. A starter line's id is fixed and derived, byte-identical on every
install; a line someone typed gets a random v4 id that exists nowhere else by
construction. So a first sync now repairs only v4 ids, and leaves the index —
and every derived id, including a carryover's archived row — alone.

## 0t. [FIXED] An edit made while a post-switch sync is in flight did not survive

**Fixed.** Production fuzz chaos seed 140. Device 0 signs into account 0 at
step 35. At step 42 its sync fails ("connection dropped"), and *during* that
run the fuzzer applies `CreateScheme { folder: 0b1b17de, name: "scheme 7911" }`
— into a folder that the same run's index write has just removed, because that
folder belongs to the account being left. The next sync, at step 43, lost the
scheme.

**The mechanism.** A pull materializes from the workspace INDEX document, and
the index write happens *after* the pull, from the pull's own result. A scheme
created while a sync is in flight is therefore not in the index the next pull
materializes from, and that pull drops the whole page. Worse than a visible
drop: applying the pull also PRUNES the live CRDT document of a scheme the
merged index neither materializes nor binds (`self.schemes.retain` in
`crdt/mod.rs`), so the content went with it. That is why the projection law
never fired — both halves agreed the page was gone.

`retained_loaded_schemes` already rescues this shape, but only for a scheme
whose `scheme_sync` binding survives in the MERGED index; here the index had
never heard of the scheme at all.

**The fix** (`sync_service/snapshot.rs`): `restore_unpublished_schemes_dropped_by_pull`
puts back a scheme the pull subtracted, and only when this device demonstrably
created it and never published it. Both halves are required:

- the server has no sequence for the document (`pull.remote_latest`), so no
  other device can ever have seen it, let alone deleted it; and
- this device still has pending edits for the document.

The second half is not belt-and-braces. `remote_latest` falls back to local
cursors when a response carries no `known_documents`, and an account switch
resets those cursors — on its own the first test would read every scheme of the
account being left as "never published" and resurrect the lot. That is the
`current.scheme_sync` dead end recorded below, which took the gate from 1
failing seed to 32.

Because the pull prunes the content document, the body cannot be read back
afterwards; it is captured *before* the pull. Cloning every scheme's items on
every sync is the most expensive thing a workspace can be asked to do, so only
the at-risk set is captured — a document with no pull cursor that still has
pending edits, which is a freshly created page and almost always nothing at all.

**A Daily page is excluded, and that exclusion is load-bearing.** The first
version restored one and broke chaos seed 127: a day outside the loaded window
is deliberately absent from the plain workspace, so putting it back breaks the
projection law from the other side — the materialized half then holds a page
the visible half does not. This is the same trap already documented on
`retained_loaded_schemes` (chaos 108, single-account 10214). The client brings
the day back from its binding through `ensure_daily_queue` when it needs it.

**Do not simply fall back to `current.scheme_sync` in
`materialize_workspace_inner`.** Tried: it took the gate from 1 failing seed to
32 (27 chaos, 5 single-account). `current` is the pre-pull workspace, so it
still lists schemes the account deleted remotely, and retaining on its binding
resurrects every one of them.

Measured at release depth (`KNOTQ_FUZZ_SEEDS=300 KNOTQ_FUZZ_STEPS=200`), both
configurations, against the same depth on `472edf1`:

| | chaos | single-account |
|---|---|---|
| `472edf1` (before) | 140 | 10290 |
| after | green | 10290 |

**Note the baseline.** 10290 fails on `472edf1` too. The note on that commit
("single-account green across all 300") was wrong, and 10290 is an unrelated
pre-existing failure — see 0u.

## 0u. [FIXED] A scheme created while a sync was in flight was never published to the account

**Fixed.** Production fuzz single-account seed 10290 — failing on `472edf1` too,
so it predates 0t. Device 3 creates "scheme 6321" while a sync is in flight
(step 158). Device 3 keeps it; every other device ignores its content document
as an orphan with no index entry, and device 3 reports `0 pending left` forever.

**The mechanism, from probes rather than inference.** Device 3's index document
held client `4019…063` clocks 10–43 that the server never received. The
in-flight `CreateScheme` WAS queued correctly (op #4 on the index document), but
the landing threw it away:

1. Landing calls `drop_unbound_pending_crdt_edits` BEFORE it adopts the run's
   workspace. At that moment the store's workspace named a stale index document
   (`712ba4a2`, which the server has never heard of) while every CRDT write went
   to the real one (`a5f399d3`). The identity is only brought back in line by
   the adoption that follows.
2. Judged against the store alone, ops #3 and #4 for the real index looked
   unbound, and were dropped — including the in-flight edit no run had sent.
3. The adoption then merged those structs into the document, so the edit
   queued afterwards (#5) was an EMPTY diff. It was pushed and cleared; the
   structs never left the device.

**The fix** (`WorkspaceStore::drop_unbound_pending_crdt_edits_at_landing`):
an edit the run never saw (`sequence >= local_edit_watermark`) is kept when the
workspace the landing is about to adopt binds its document. The watermark was
already threaded into `clear_pushed_edits` and unused.

**The watermark gate is load-bearing.** The first version kept any edit the
incoming workspace bound, and broke single-account seed 10208: a device's
pre-canonical index edits that its run had deliberately discarded (seqs 1–9,
"0 pending left") came back, were re-sent later as superseded deltas, and a
Daily line was lost on the server. An edit that WAS in the run keeps the
store-only rule; the run already decided what to do with it.

**Still worth knowing:** the store's plain workspace naming a different index
document than its CRDT between landings is itself odd, and is what made this
possible. It is harmless now that nothing destructive judges by it alone, but it
is the next thing to look at if a similar shape turns up.

## 0v. [FIXED] Crash recovery populated documents from a base this device never wrote

**Fixed.** Production fuzz chaos seed 289 — it stopped the first v0.57.0 release
build at the sync gate, and it fails identically on `472edf1`. Two schemes and a
folder another device created were deleted for the whole account.

1. Device 4's first sync pulls the account, persists the pulled workspace and
   CRDT (already re-keyed to the account's index identity), then its push
   drops. The run never lands; the store keeps its own install identity.
2. The device crashes mid-save. The save marker's base is "whatever the plain
   files held" — the account's pulled content.
3. On relaunch the store's index document is unpopulated, so
   `recover_workspace_save` took the joining-with-local-content branch:
   populate from `base`, write the plain workspace on top. The account's
   schemes and folder became a local deletion, pushed at the next sync.

**The fix** (`WorkspaceStore::recover_workspace_save`): a base under a
different workspace index identity than the plain workspace is still used to
notice what the plain files changed, but never as a POPULATION base, for the
index or for a scheme document. Populating from it is what turns someone else's
content into this device's deletions and reverts.

**Replacing the foreign base outright is the wrong fix** — tried, and it broke
chaos 190 and 299: recovery then sees no change at all, writes nothing, and the
documents stay behind the plain files, so the projection law fails on launch.

## 0w. The nightly deep gate never ran, and at its configured depth it is red

**Found 2026-09-24 while debugging eight consecutive red nightlies.** The red
was not a sync bug. `sync-fuzz.yml` installed only `pkg-config` and
`libdbus-1-dev`, but its last step builds `knotq-app`, which links GPUI against
the X/Wayland stack, so that step never linked:

    rust-lld: error: unable to find library -lxcb
    rust-lld: error: unable to find library -lxkbcommon
    rust-lld: error: unable to find library -lxkbcommon-x11

It has failed that way on every run since it was added on 2026-09-15
(`8f7ad01`); the 09-15 and 09-16 "successes" were 10-second cache-marker skips,
not runs. Fixed by using `actions/linux-deps` — the definition
`sync-stress/action.yml` already builds this same crate under. (The WebSocket
job was separately red 09-17 to 09-19 on a real bug, `offline_device_join`'s
"the joining device's own scheme was lost"; #39 fixed that on 09-20.)

**So the depth in that file had never executed once.** It is
`KNOTQ_FUZZ_SEEDS=400 KNOTQ_FUZZ_STEPS=300`, written when the step was added
and never run. The release-depth gate these notes describe is 300 x 200, so the
file raises *both* axes. Steps drive simulated midnights (`actions.rs`: "the
device's day moves on"), so 300 steps reaches materially more day rollovers.

**Measured at 400 x 300, one seed per process: 7 failing seeds of 800.** The
parallel sweep and the one-at-a-time replay agree exactly here. All 7 fail
**identically on `7820f3a`**, the commit before v0.57.0's `fe924e9`, so none of
them is a regression from that work — this is pre-existing backlog that the
untried depth exposes.

| Seed | Config | 120 | 200 | 300 | What it reports |
|---|---|---|---|---|---|
| 194 | chaos | pass | pass | **fail** | settle lost the Daily Queue binding for 2026-09-02 (`bb7c6776…`) |
| 332 | chaos | **fail** | **fail** | **fail** | step 70: device 3's sync lost item `…0402` in `ba929b98…` "Daily 2026-09-15" that no device deleted |
| 389 | chaos | pass | **fail** | **fail** | step 155: device 1's workspace has 13 items in `ba929b98…`, its CRDT has 14 |
| 10054 | single | pass | pass | **fail** | step 274: device 1's sync lost item `88b70256…` in `1d98a5db…` "Daily 2026-09-17" that no device deleted |
| 10117 | single | pass | pass | **fail** | steps 269/271: device 2, item `…4009` in `1ec12563…` — the document holds the workspace's text applied twice |
| 10209 | single | pass | pass | **FIXED** | steps 285/287/290: device 3, `1ec12563…` has 22 items to the CRDT's 21; `473f9fd3…` repeated in the workspace |
| 10350 | single | — | **FIXED** | **FIXED** | step 202: device 3's sync lost item `92b72501…` in `ba929b98…` "renamed 7192" that no device deleted |

Four (194, 10054, 10117, 10209) are inside the swept seed ranges and are
exposed purely by 200 -> 300 steps. Three (332, 389, 10350) are outside them
and are exposed by 300 -> 400 seeds; 332 fails even at the default 120.

**Every one of them lands on a Daily Queue page.** `1ec12563…`, `1d98a5db…`,
`ba929b98…` and `bb7c6776…` are all v8 (derived) ids — including 10350's, which
the fuzzer had renamed to "scheme 7192" and which still is one. 10209's symptom
is the one `daily_queue_carryover_command` already names in a comment ("two
rows with one id in the page, which no document can represent", single-account
seed 10095): the `previous_ids.contains(&displaced.id)` guard there covers the
source day already holding the archive id, and these reach the same end state
by some other route. Start with repeated rollovers of one page, not the landing.

**The 300 x 200 baseline these notes call green is not green either.**
Single-account seed **10175** fails at 200 steps (and passes at 120 and 300),
inside the 10000–10299 range the note above says passes "by sweep and by
one-at-a-time replay". It is not content loss and not a Daily page: devices 0
and 3 diverge at settle over `70db3d43…`'s name and colour, `2b1646c2…`'s
source, and root-child ordering. It fails identically on `7820f3a`, so it too
predates v0.57.0 — the note was wrong, not the code. Measuring the baseline
instead of trusting the sentence about it is the recurring lesson here.

### 10209 and 10350: fixed 2026-09-24 — a scheme held two rows with one id

An `items_by_id` map has one entry per id, so a scheme whose plain copy holds an
id twice has no CRDT representation at all: the halves disagree from that moment
on. `insert_item` in `desktop/commands/src/apply/item.rs` inserted
unconditionally, and the case that reached it was **an undo landing after a sync
had already restored the row** — 10209: device 3 deletes a Daily row at step 228,
a later sync re-materializes it, and the undo at step 285 adds a second copy
(22 rows against the document's 21 from then on). That is TODO item 4 (undo not
surviving a sync) turning into corruption rather than just a stale undo.

The insert now restores the row's value in place and returns the displaced value
as its inverse, so redo stays coherent. Pinned by
`inserting_an_id_the_scheme_already_holds_restores_it_in_place`
(`desktop/commands/tests/item_cmds.rs`). It fixes 10350 as well.

### 194, 389 and 10054: the landing's placement reconcile is gated on a visible change

Traced 2026-09-24 on chaos 389. A line carried from one Daily page to another
keeps its id, so the move is a tombstone in the source document plus a live
insert in the destination. `reconcile_item_placements` is what deletes a losing
cross-document copy, and the landing only calls it when
`adopted || item_repairs`.

That gate is **circular**: materialization hands a line live in two documents to
the lowest scheme id, so a remote update that reintroduces a live copy in the
*other* document changes nothing visible — `adopted` is false precisely because
the duplicate is hidden. It stays hidden until the visible copy is deleted, and
then the hidden copy is all that is left. In 389 device 1 carries `…4004` out of
`ba929b98` at step 102 (verified: right after the carryover the item is live in
one document only, so the carryover's tombstone is correct), a later landing
quietly restores a live copy in `ba929b98`, and the delete at step 155 reveals
it — 13 visible rows against the document's 14. In production this is "a line I
deleted came back in another scheme".

**The obvious fix, and a correction about what it costs.** Gating on
`adopted || item_repairs || remote_updates_applied > 0` fixes chaos 194, chaos
389 and single-account 10054. Perf budgets stay green. It also makes chaos 48 and
238 report "sync lost item …0402 / …4009 that no device deleted", and that was
first written up here as the change losing content. **That was wrong**, and the
same misreading as 332's: the oracle's wording is about a *passive placement
change*, not a deletion.

Measured per push (does applying it to the server's base add or remove the item
from each document):

- Under the spike, every REMOVE in 48 and 238 is paired with an ADD **in the same
  step**. The row is never absent from the account.
- On unmodified `main`, where **both seeds pass**, the same item performs the same
  cross-document moves — 48's transitions are identical (steps 81, 126, 203, 220,
  235), 238's roll-forward lands at step 205.

So the oscillation **pre-exists on main and simply is not reported there**. The
spike does not create it; it changes the trajectory enough that the attribution
oracle observes one of the transitions. Nothing is lost either way.

The honest trade is therefore: the spike fixes three genuine projection-law
divergences (what the user sees vs. what their own documents hold) and converts
two *hidden* placement bugs into visible ones. It still leaves the nightly red —
red on 48/238 instead of 194/389/10054 — so it does not get the gate green on its
own. Kept on `spike/landing-placement-reconcile-gate`; the reason not to merge it
is now "it does not finish the job", not "it loses data".

**What the trade-off means, and what is NOT yet known.** Widening the gate
makes `reconcile_item_placements` run on landings it used to skip, and that
reconcile *deletes* what it judges to be the losing copy of a cross-document
duplicate. In 48 and 238 it deletes a row nothing deleted. So the reconcile's
"losing copy" judgement is wrong in at least some cases the old gate simply never
showed it — but **why** is unverified, and two plausible-looking explanations are
already ruled out:

- *Not* devices installing on different days and seeding the same fixed starter
  ids into different day pages: every fuzz device installs with
  `today = 2026-09-15` (`World::today`), so the starter ids land in the same
  documents on all of them.
- *Not* item `…0402` being live in two documents around 332's loss: probing
  `documents_holding_item` on every projection reading for the whole seed reports
  **no** multi-document holder at any step.

So do not start from "a fixed starter id is live in two day documents". 332's
loss is observed in *server* state by the attribution oracle (the
`18446744073709551615` pseudo-device), not as a projection divergence on any
device.

### Item skeleton structs alias across documents — found 2026-09-24

Following 332 to the server gave a much sharper lead. At step 53 **device 0**
moves item `…0402` from `2b1646c2` (Daily 09-14) into `ba929b98` (Daily 09-15).
At step 70 **device 3** — which never held that row in `ba929b98` — pushes, and
the server loses it. A delta cannot remove content its author never saw, so the
two documents must share struct identity. They do:

`stable_item_seed_client_id` hashes the **item id alone**. Every other derived
identity in `crdt::encoding` namespaces by document
(`stable_item_creation_client_id`, `stable_scheme_population_client_id` both hash
the `DocumentId`), so this looks like an oversight. `build_item_skeleton_update`
then builds the skeleton on a fresh document, so the clocks start at 0 as well —
the same item id in two scheme documents occupies the **identical
`(clientID, clock)` range**.

Pinned as `item_skeleton_structs_must_not_alias_across_documents`
(`shared/sync/src/crdt/tests/merge.rs`, `#[ignore]`d). Delete the row in one
document, merge that document's state into the other, and the other's row goes
too:

```text
B items before=1 after=0
```

One id legitimately lives in two scheme documents whenever a line moves between
schemes — a carryover does it unprompted — so this is reachable by anything that
lets one document's delete set meet another's structs: a base rebuild, an epoch
squash, a server compaction (332 has one at step 47), a reseed. It is the same
shape as the cross-*account* struct aliasing already fixed by re-identifying the
workspace document; the cross-*document* half is still open.

**The aliasing is exercised in the real run, not just synthetically.** Probing
every push in 332 for item `…0402`'s seed clientID (`7747370098865841`) — the
daily document ids are `ea4f07d4` for 09-14 and `f9bc2620` for 09-15 — shows
**both** day documents pushing structs under that one clientID, from **step 27**
onward. That is well before device 0's move at step 53, so the two documents hold
the same struct identity independently; the move is not what creates it.

**Honest limits.** The aliasing is proven, and its presence in two documents'
real pushes is proven. That it is what *deletes* the row at step 70 is still not:
the step-69/70 push does include `f9bc2620`, but without inserted structs for
that clientID. `client_ranges` reports an update's *inserted* ranges only, not its
delete set, so "no structs" is consistent with "carries a delete over that
client's range" — which is the hypothesis — but does not demonstrate it.

**That measurement was taken, and it says the aliasing is NOT 332's cause.**
Probing every push for whether applying it to the server's base adds or removes
`…0402` from each document gives a **placement oscillation**, not a deletion
(`ea4f07d4` = Daily 09-14, `f9bc2620` = Daily 09-15):

| step | device | effect |
|---|---|---|
| 8 | — | ADDS to 09-14 (the starter seed) |
| 27 | — | REMOVES from 09-14, ADDS to 09-15 (the roll-forward) |
| 54 | device 1 | **ADDS back to 09-14** |
| 55 | device 0 | REMOVES from 09-15 |
| 58 | device 3 | ADDS to 09-14 again |
| 87 | — | REMOVES from 09-14, ADDS to 09-15 |
| 166 | — | REMOVES from 09-15 |

No push at step 70 removes the row from any document, and the row is never
absent from the account — it is in 09-14 the whole time the oracle calls it lost.
So **332 is not "an item was deleted"; it is "an item's placement ping-pongs
between two Daily documents across devices' pushes"**, and the step-70 report is
one transition of that oscillation seen by the attribution oracle.

Devices 1 and 3 still hold 09-14's pre-roll-forward view — they never saw the
step-27 carryover — and their pushes put the row back there. A plain Yjs
re-insert cannot beat a tombstone, so the resurrection is going through something
that re-marks presence: the `resurrect:` presence tags in `read_stored_item`, the
"an ordinary scheme write preserves raw-only copies on purpose" rule, or the
`moved_edits` reassert (whose own comment warns about exactly this ping-pong
shape: "two devices each re-assert their retained snapshot on every landing").
**That is where to look next** — not at the skeleton aliasing.

The aliasing in the section above is still real and still worth fixing on its own
merits; it is simply not what 332 is. Also disproved along the way: item `…0402` is never live in two
documents on any device at any step in 332 (probed across all devices, all 300
steps), so the dedupe-picks-the-wrong-copy story is not it either.

**Why it is not fixed here.** Adding the document to the skeleton's clientID
changes struct identity. Existing documents on users' disks hold skeletons under
the old id, so a new build authoring a different one creates a *second* container
for the same item — precisely what the deterministic skeleton exists to prevent —
and older clients would keep writing the old one. That is an incompatible format
change: it needs an encoding-version bump (`ITEM_CREATION_ENCODING_VERSION` and
`scheme_population_encoding_is_pinned` are the existing precedent), a migration,
and a decision about mixed fleets. Not something to bolt on.

The gate and the reconcile's delete decision are coupled, so the gate cannot be
widened until that decision is trustworthy — and the direction that loses content
is the worse of the two failure classes.

### 10117's mechanism, confirmed 2026-09-24

`build_item_creation_update` authors an item's creation text under
`stable_item_creation_client_id(document, item_id, content)` — which hashes the
**content**. That content key is load-bearing: it is what makes N devices
creating the same line with the same text dedupe into one copy (seed 10013,
"three devices carrying the same line over produced it three times over").

When two devices' first write of the same item id carries *different* content,
they get different clientIDs, so yrs integrates both runs and the Text holds one
after the other. Device 2 edits the starter Daily line `…4009`
(`starter.daily.next_one_there`) before its first sync, so its creation seed
carries "agent 7277…" while another device's carries the original, and the
merged document holds both. That is the divergence the projection law reports.

Pinned as `concurrent_same_item_creation_with_different_content_doubles_the_text`
in `shared/sync/src/crdt/tests/merge.rs`, `#[ignore]`d as an open gap. It fails
in one line, with no fuzz harness:

```text
merged text = "When you finish a task the next one is right there\
               agent 7277When you finish a task the next one is right there"
```

Note that `reconcile_content_shadow` already exists to paper over this, and only
fires when the actual content is *exactly* the shadow twice — which the edited
case never is.

**Why it is not fixed here.** Dropping the content key re-breaks 10013, and no
merge-time rule can help: if two replicas integrate different seed runs there is
nothing to prefer until both are present. The fix has to be a *convergent
repair* — every replica independently picks the same surviving run (e.g. from an
additive per-seed marker in the item map, so the choice is a pure function of
the document) and removes the rest. That is a wire-format addition and a change
to the most delicate merge path in the codebase, so it wants its own change
driven by the fuzzers, not a patch bolted onto the CI fix that exposed it.

**Reproduce (≈2-3s each):**

```sh
KNOTQ_REPRO_PLAIN=1 KNOTQ_REPRO_SEED=10117 KNOTQ_FUZZ_STEPS=300 \
  cargo test --release -p knotq-app replay_production_seed -- --ignored --nocapture
# chaos seeds: drop KNOTQ_REPRO_PLAIN
```

**Where this stands after 2026-09-24.** 10209 and 10350 are **fixed** (one row
per id). 194, 389 and 10054 are traced to the landing's placement-reconcile gate,
with a patch on `spike/landing-placement-reconcile-gate` that fixes them and
surfaces two pre-existing oscillations (48, 238) rather than causing them — see
the correction below. 10117 is root-caused and pinned. 332, 48 and 238 are all
the same thing: **a row's placement never converges across devices, and each push
publishes the pusher's own view.**

The one number worth carrying forward: in 332 the devices disagree permanently —
at step 70 device 3's workspace says the row is in `2b1646c2` while device 0's
says `ba929b98`, and by step 78 device 0's says it is in *no scheme at all*. That
is the bug to fix; the oracle reports are downstream of it.

Two more measurements, so the next attempt does not repeat them:

- **It is not the loaded window.** The obvious explanation — placement is resolved
  over each device's *materialized* schemes, and devices page in different days
  (`dedupe_materialized_items`' own comment says off-window Daily pages do not
  participate) — is wrong here. At step 70 devices 0 and 3 both have 09-14, 09-15
  **and** 09-16 loaded and still disagree.
- **Nothing is lost at the account level.** Replaying the per-push add/remove
  ledger for the whole run, the row's final state is always some document
  (332 → `ea4f07d4`, 48 → `cbf12972`, 238 → `6eff952f`). The momentary "in no
  document" points are within-step artifacts: a push's REMOVE from the source is
  ordered before its ADD to the destination. A *device* can still show the row
  nowhere for a while (332, device 0, step 78), which is user-visible on its own.

Since each device has the row live in exactly one document and different devices
pick different ones, their documents hold mutually inconsistent tombstone sets,
and each landing re-asserts the pusher's visible placement back into its own
documents. `reconcile_item_placements` is what would correct the visible copy —
which is why the gate above is implicated — but its winner (lowest scheme id) is
computed per device, so widening the gate alone does not make the fleet agree.

332 is the one remaining seed with no mechanism yet; 48 and 238 are the same
shape but only appear if that gate lands. 332's violation is at step 70, before
any device in that seed crosses an account boundary, so it is not the known
account-switch exclusion (0i), and two hypotheses for it are already disproved
(see the gate section above — read those before re-deriving them).

**Decision still to make.** The link fix alone turns the nightly from "red
because it cannot build" into "red because it finds real seeds". Either
drain them and keep 400 x 300, or bring the file down to a depth that is
actually green and raise it deliberately afterwards — which is what the note
above ("raising the gate's depth is worth doing only once the sweep is green")
already says, written while this file quietly specified a higher one.


## The parallel sweep can miss a failing seed

289 failed on the CI runner and on every single-seed replay, but passed in three
local 300-seed sweeps on identical code. The sweep runs seeds on worker threads
alongside the other sweep; something those share (not the id stream, which is
thread-local and reset per seed) changes a seed's outcome. Until that is found,
treat a green sweep as necessary rather than sufficient: the deterministic
answer is one replay per seed.

```sh
BIN=$(ls -t target/release/deps/knotq-* | grep -vE '\.(d|rlib|rmeta)$' | head -1)
KNOTQ_REPRO_SEED=<n> KNOTQ_FUZZ_STEPS=200 $BIN replay_production_seed --ignored \
  --exact app::sync_service::production_fuzz::replay_production_seed
```

As of 0v, all 600 release-depth seeds (chaos 1–300, single-account
10000–10299) pass that way, one at a time.

## 0i-b. A device that has switched accounts still breaks the projection law

**Open, and the projection law's one documented exclusion.** The law
([the section at the top](#how-this-class-of-bug-is-found-now-the-projection-law))
holds for every device in the ordinary multi-device case, and through crashes,
relaunches, dropped connections, server faults and compaction sweeps. It does
**not** hold for a device whose data directory has crossed an account
boundary: at the PR gate's depth (`KNOTQ_FUZZ_SEEDS=128 KNOTQ_FUZZ_STEPS=200`)
the chaos configuration reported it on 10 of its first ~60 seeds (16, 26, 29,
39, 42, 47, 49, 51, 52, 53), every one of them on a device that had signed into
a second account earlier in the run.

**What it looks like** (seed 16, device 0, which signs into account 1 at step
110 and crashes before its next save at 117): from step 119 onward the plain
workspace and the documents disagree in *both* directions at once — schemes
where the documents hold items the workspace does not
(`00000000-…-0102`: 10 plain vs 13 in the CRDT), schemes where the workspace
holds one the documents do not (`…-0103`: 10 vs 9), and item fields that
differ outright. The shape says the documents still carry the source account's
history while the plain files describe the destination account's view.

**Reproduce:**

```sh
KNOTQ_REPRO_SEED=16 KNOTQ_FUZZ_STEPS=200 \
  cargo test --release -p knotq-app --bin knotq replay_production_seed \
  -- --ignored --nocapture
```

then re-run with the exclusion lifted (delete the `projection_excused` guard in
`World::check_projection`). `KNOTQ_CHECK_DISK=1` shows the same divergence in
the data directory.

**Why it is excused rather than fixed here:** a switch is a data-lineage
boundary — the attribution oracle skips its own check across it for the same
reason (`account_changed` in `World::sync`) — and the switch path
(`queue_account_switch_reseed`, `reset_for_account_change`,
`adopt_sync_workspace_identity`'s disjointness guard) is the most intricate
corner of the sync service. The exclusion is **per device and permanent for the
rest of the run**, so nothing else is weakened: a device that never switches
accounts is still held to the law on every local step, landing and relaunch,
in both fuzz configurations.

**Not the same thing as the account-switch data loss the oracle finds.** With
the exclusion in place, chaos seeds 26 and 112 still fail at that depth — but
on the *existing* no-silent-loss oracle, not on this law: "sync lost scheme …
that no device deleted". That is a separate, pre-existing account-switch
data-loss bug (commit `4abec2a` fails seed 112 too, without any of this
session's changes), at a depth the default corpus never samples.

**What seed 112 shows, as of 2026-09-20 — start here.** Device 4 creates
"scheme 7563" on account 0 at step 71. Device 2 lives on account 1, signs into
account 0 at step 125 and syncs at 126 (so it should hold 7563), has a sync
fail at 165, crashes at 166, relaunches at 189, and at 191 pushes 6 documents —
after which the *server* no longer has 7563, and device 4 loses it at 194.

Device 2 does **not** break the projection law at any point (a traced run now
prints `PROJECTION (excused)` lines for a switched device, and seed 112 emits
none), so its plain workspace and its own documents agree: its index genuinely
does not hold 7563 by then. The question is therefore not "how did device 2's
two halves diverge" but **"how did device 2's push delete an entry its document
never carried a tombstone for"** — which points at the account-switch re-seed
(`queue_account_switch_reseed`) and workspace-document re-identification
(`adopt_sync_workspace_identity` / `reidentify_workspace_document`) publishing
this device's index as the account's, rather than merging into it. A plain Yjs
merge cannot remove an entry the pusher never saw; a re-key or a full re-seed
can.

Chaos seed 26 is the same oracle violation on a device that also switched
accounts, and is worth replaying alongside it:

```sh
KNOTQ_REPRO_SEED=112 KNOTQ_FUZZ_STEPS=200 KNOTQ_FUZZ_TRACE=1 \
  cargo test --release -p knotq-app replay_production_seed -- --ignored --nocapture
```

**Where to start:** the divergence is already on disk, so check the halves
after each write in `sync_snapshot_in` during the switch — the run's
`save_workspace` / `save_crdt_state` pair, the post-push re-materialization,
and `queue_account_switch_reseed`'s queued snapshots — rather than tracing the
in-memory store. The most likely shape, given the two-directional difference,
is that the re-seed publishes the pre-switch document set while the pulled
index describes the destination's.


## 0j. [FIXED] Occurrence completions survived in a document after the line stopped being a checkbox

**Found by the projection law** in the 128-seed production fuzz (single-account
seed 10002): a `Numbered` line's plain copy held one default `OccurrenceState`
while its document held six, five of them completions of recurring
occurrences.

**Root cause, the same shape as 0d:** dates, recurrence and occurrence state
are only valid on a checkbox, and `Item::enforce_marker_constraints` strips
them everywhere the plain workspace is written. A *document* is not written
that way — it is the merge of whatever every replica ever wrote — so it can
hold a combination the model does not allow, and the plain side keeps dropping
what the document keeps holding.

**Fix:** the law compares what the app would *show*, normalizing the
materialized item with `enforce_marker_constraints` inside
`projection::divergences` before the comparison.

Materialization itself deliberately does **not** normalize, and must not: a
normalizing read makes every sync see such a line as changed and rewrite it,
and under compaction that churn lost other devices' folder and archive edits on
the server. That is pinned by
`a_line_reads_back_exactly_as_written_so_syncing_queues_nothing_more`
(`shared/sync/tests/concurrent_item_field_edits.rs`, from compaction fuzz seeds
114 and 246) — normalizing on read was tried here and that test caught it
immediately. The storage forms may differ; what the two halves *describe* may
not.

## 0k. [FIXED] Un-completing a recurring occurrence left a husk the sync path pruned

**Found by the projection law** in the 128-seed production fuzz (single-account
seed 10005, step 163): a checkbox's plain copy held an `OccurrenceState` for
`2026-09-15` with `progress: 0` that its own scheme document did not.

**Root cause:** `Item::state_for_occurrence_mut` creates an entry on demand, and
`ToggleOccurrence` toggled `progress` without normalizing afterwards. Completing
an occurrence and un-completing it therefore left a default entry behind — which
says exactly what *no* entry says, since `state_for_occurrence` returns the
default for a missing one. It would have been harmless bookkeeping except that
the sync path normalizes the copy it writes into the CRDT documents
(`workspace_for_background_sync` runs `normalize_item_markers`, which for a
checkbox ends in `normalize_state`), so the husk existed in the plain half only.

**Fix:** `toggle_occurrence` normalizes after the toggle
(`desktop/commands/src/apply/item.rs`), and `Item::normalize_state` now
*reports* whether it dropped anything so `enforce_marker_constraints` no longer
returns "unchanged" for a repair it just made — that return value is what tells
the sync service which scheme documents to rewrite (see 0e).

**Regression:** `un_completing_a_recurring_occurrence_leaves_no_husk_behind`
(`desktop/commands/tests/item_cmds.rs`).


## 0l. [FIXED] The two halves of the data directory could part company with no save in progress

**Found by the projection law** in the 128-seed production fuzz (chaos seed 89,
step 149) after teaching `World::crash` to check the law once the relaunch is
done — a crash is precisely where the halves are most likely to split, so it
was the obvious place for the law to be checked and it was the one state move
that did not check it.

**What happened:** device 0 edited a line (step 140) and moved a scheme (step
145) without saving, then synced at 149 and the push failed with a dropped
connection. On relaunch its documents held both edits and its plain files held
neither — `KNOTQ_CHECK_DISK=1` shows the split is already on disk before the
process starts.

**Root cause:** a sync run persists the *post-push* document states on their own
(`sync_service/snapshot.rs`, just before `push_result?`) because the push's own
self-heal may have repopulated a schema-less document and that identity has to
survive a restart. That write is not paired with a workspace save, and it
happens *after* the run clears `workspace_save_recovery` — so when the push then
fails, the documents on disk are ahead of the plain files with no save in
progress and no marker to notice it. Item 2's recovery only runs when a marker
is present, so nothing repaired it; the device then re-materialized the
documents' values during a later landing, which is indistinguishable from a
remote change nobody made.

**Fix:** the marker-independent half of recovery became
`WorkspaceStore::reconcile_workspace_from_documents`, and **every** launch runs
it — with a marker through `recover_workspace_save` (which still turns the
plain-file delta into CRDT operations first), without one on its own. It only
ever adopts what the documents hold: a scheme with no document, an unseeded
workspace document and an empty document all keep the plain content, so a device
that has never synced passes through untouched.

**Verified:** chaos seed 89 at `KNOTQ_FUZZ_STEPS=200`, and the law is now
checked after every crash-and-relaunch in both fuzz configurations.


## 0m. [FIXED] A second carryover could put two rows with one id on a day

**Found by the projection law** (single-account fuzz seed 10095, chaos seed
76): a daily page held the same `ItemId` twice, which no CRDT document can
represent, so the plain workspace and the documents disagreed from then on.

**Root cause:** a carryover leaves a deterministic, date-scoped archive copy of
each carried row on the source day (`daily_queue_displaced_item_id`). Because
the id is deterministic, another device's carryover of the same row can merge
into the source day while this device still sees the live row — and the usual
delete + insert pair then inserts a *second* copy of an id the page already
holds.

**Fix:** `daily_queue_carryover_command` skips the archive insert when the
source day already holds that id. The live row still leaves, which is the part
that matters. Regression:
`carryover_does_not_add_a_second_archive_copy_of_a_row`.


## 0n. [FIXED] A workspace read back out of the documents was not put in normal form

**Found by the projection law** (single-account fuzz seed 10024): a line showed
a start date while its marker was `Numbered` — a combination
`enforce_marker_constraints` exists to prevent. Its document held no date, so
the two halves disagreed, permanently.

**Root cause, the general form of 0d/0e/0j/0k:** the plain workspace is the
canonical copy and every path that writes it normalizes; materialization
deliberately does not (0j). So every point where the plain half is *re-derived
from the documents* has to normalize on the way in, and three did not — the
post-push re-materialization and the post-squash adoption in
`sync_service/snapshot.rs`, and `WorkspaceStore::reconcile_item_placements`.
The next sync snapshot then normalized the copy it wrote into the documents
(`overlay_current_workspace_for_sync`) while the visible workspace kept the
value the model forbids.

**Fix:** all three normalize what they adopt, as `recover_workspace_save` and
`reconcile_workspace_from_documents` already do. `ItemFields::apply` — the
reassert journal's field-mask merge — also ends in `enforce_marker_constraints`
now: carrying fields one at a time can assemble a combination no single writer
would have produced.


## 0o. [FIXED] Deleting a line could bring back a hidden copy of it in another scheme

**Found by the projection law** (chaos seeds 39, 42, 52), and a genuine
user-visible data bug rather than only a divergence.

Each scheme is its own CRDT document, so a line moved between schemes is a
tombstone in one and a fresh copy in the other. When two devices move the same
line to different schemes, both documents end up holding a live copy.
`dedupe_materialized_items` hides all but one — deterministically, by lowest
scheme id — so the user sees a single line. But the losing copy is still live
CRDT history, and `merge_raw_only_items` deliberately keeps it: delete the
visible line and the hidden one becomes the winner, so a line the user deleted
reappears in another scheme days later.

**Fix:** stop hiding and start resolving. `dedupe_materialized_items` now
reports the copies it hid, and `reconcile_item_placements` — which every
landing that adopted anything runs — deletes them from their documents,
tombstoning them explicitly (an ordinary scheme write preserves raw-only copies
on purpose; only a named deletion retires one). "A line is live in at most one
document" becomes an enforced invariant instead of a display-time tie-break.

**This cannot lose the line.** The winner is a *minimum* over the schemes a
replica can see, so the copy in the globally lowest scheme id is never a loser
anywhere; whatever subset of schemes each replica has loaded, at least one copy
always survives.

## 0p. [FIXED] Normalization could destroy a scheme, and a permanent delete could leave no evidence

Two halves of one rule: **normalization repairs structure; only a real deletion
destroys content, and a real deletion leaves evidence.**

`normalize_one_level_folders` deleted any scheme the folder tree did not
mention. That is destructive twice over — the scheme goes, and because the
workspace index is written from this workspace, the drop is *published* to the
account as an authoritative deletion, so every other device loses it and its
document is left on the server as an orphan with no index entry. An
unreferenced scheme is now re-homed under the root instead, the same choice the
folder walk already makes for a stranded folder. Regression:
`normalize_rehomes_an_unreferenced_scheme_rather_than_deleting_it`.

`permanently_delete_scheme` wrote its tombstone only when the scheme had a
recorded restore origin, so a scheme archived without one (archived with its
folder, or an origin pruned by an earlier normalization) was destroyed with
nothing to say so. The tombstone is not trash bookkeeping — it is the evidence
that an id was destroyed, and a stale replica merges the node back to life
without it. It is now always written, falling back to the root as the origin
folder.

Neither is enough on its own to close the remaining account-switch scheme loss
(0i) — both were verified against chaos seeds 12, 14, 26, 112 and 113, which
still fail — but both are real holes in the rule those failures violate, and
the rule is what any fix for 0i has to rest on.

**Rejected on the way, deliberately:** "a local index write never removes a node
entry". It does fix seeds 12, 14 and 112, but it retains entries for schemes
whose documents this replica does not hold — phantoms that
`queue_local_only_documents_before_pull` then snapshots from an empty
materialized scheme on every sync, so the account never goes quiet (seed 10000
stopped reaching a squash window at all) and an empty snapshot could overwrite
the real content server-side. Narrowing it to "nodes whose document this
replica holds" makes it correct and useless: `self.schemes` is pruned to the
workspace right after the index write, so the narrow set is what
`retained_scheme_ids` already keeps. A fix for 0i has to establish *why* the
pushing device's index lost the entry, not stop it from writing what it
believes.


## 0q. [FIXED] Crash recovery could publish a deletion of another device's schemes to the whole account

**The largest remaining loss, and the one the oracle kept reporting** (chaos
seeds 12, 14, 26, 112, 113 — 112 fails on `4abec2a` too).

**Mechanism.** The workspace index belongs to the *account*, and it is written
whole: `sync_string_map` removes every key the content it is given does not
mention. `recover_workspace_save` wrote it from the recovered **plain**
workspace, and a scheme write re-emits the index whenever the document set
changed, so a relaunch whose plain workspace was missing schemes — an
interrupted save, a pull that never landed, a device mid-account-switch —
published their deletion to every device. The scheme's document stayed on the
server as an orphan nothing could address (`sync: ignored 1 orphan
document(s)`; that message is the symptom, and its first appearance dates the
loss).

**Fix: recovery is additive.** It now starts from what the documents hold
(`reconcile_workspace_from_documents`), lays the schemes the plain files
actually changed since the recovery base back on top, writes *those* into the
documents, and finishes by reconciling again — so it ends showing exactly what
its documents hold, like every other launch. The index is written from the
plain workspace only when the document has no population at all and this
workspace is the only thing that can give it one (TODO 2's joining device).

The cost is that a folder rename or archive that reached the plain files but
not the documents in the moment before a crash is re-read from the documents
instead of being recovered. That is a lost keystroke; the alternative was
losing another device's scheme for the whole account.

**New diagnostic:** `sync: workspace index write removes N node entr(ies): …`
(`crdt/workspace_index.rs`). Removing a node entry is the most destructive
thing this codebase does and it is *sometimes* right, so the writer reports
rather than refuses — which is what turns "a scheme vanished for everyone" into
a named step and device.

**Measured after this fix:** at `KNOTQ_FUZZ_SEEDS=128 KNOTQ_FUZZ_STEPS=200` the
whole chaos sweep is green and the single-account sweep fails one seed, against
**52 failing seeds on `4abec2a`**.


## 0r. The last CI-depth failure: a Daily page's colour, single-account seed 10105

**2026-09-22: passes.** Seed 10105 is inside the release-depth single-account
sweep (seeds 10000–10299 at 200 steps), which is green end to end after 0t and
0u. Nobody has traced WHY it stopped failing, so this stays listed until someone
does — a fix that arrives as a side effect is worth one replay with the trace
to confirm it is the same mechanism and not a masked one.

**Open, and the only seed failing the 128×200 sweep.** Not content loss — one
scheme metadata field.

```sh
KNOTQ_REPRO_PLAIN=1 KNOTQ_REPRO_SEED=10105 KNOTQ_FUZZ_STEPS=200 \
  cargo test --release -p knotq-app replay_production_seed -- --ignored --nocapture
```

Device 3 recolours the Daily page `1ec12563…` to 1 at step 87. Device 1 first
sees that page in the landing at step 192 (7 remote updates applied), and from
then on its plain workspace says colour 0 — the value
`DAILY_QUEUE_COLOR_INDEX` gave the copy it created locally — while its own
index document says 1. The projection law reports it on every subsequent step;
it does not heal.

Colour comes from the workspace index entry in `materialize_workspace_inner`,
so the CRDT side is unambiguous. The question is why the landing's replace
(`replace_workspace_from_sync_result` → `store.replace_from_sync`) leaves the
visible copy at 0 — whether the run's returned workspace already carried 0, or
the store's replace keeps the local scheme's metadata. Start by printing the
colour of that scheme at each boundary inside the step-192 landing.

**Seen in the same trace, and probably worth more:** `sync: workspace index
write removes 1 node entr(ies): 00000000-…-0101` during a *sync run* (not a
relaunch — 0q fixed that path). A second write site is still handing the index
writer a workspace that is missing a node. The diagnostic names the moment; the
work is finding what that workspace is and why it is short.

**Harness note:** the excused-device projection reading is taken whether or not
`KNOTQ_FUZZ_TRACE` is set. It flushes the store, which consumes ids off the
deterministic stream, so taking it only when tracing made a traced run a
different scenario from the failure it was meant to explain — seeds 12 and 113
passed when traced and failed when not.


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
