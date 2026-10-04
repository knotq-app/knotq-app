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

## The five laws, and why they compose

The projection law above is one of five. Stating them together is what turns a
pile of seed fixes into an argument, because the thing we actually want is a
single global property:

> **No intent the user applied and the device acknowledged is ever lost.**

That is not directly testable — "ever" quantifies over every interleaving of
edits, syncs, crashes, interruptions and clock changes. It decomposes into five
local properties that *are* testable, and whose conjunction implies it.

**L1 — Projection.** A device's plain `Workspace` is exactly what its own CRDT
documents materialize to. *Checked:* `desktop/state/tests/projection_law.rs`,
every step of the production fuzzer, and `KNOTQ_CHECK_DISK=1` against the real
data directory. *Stated in:* `shared/sync/src/projection.rs`.

**L2 — Intent durability.** When a command is acknowledged to the UI, its
effect is already in the durable CRDT. *Checked:* the production fuzzer drives
real `AppState` and real disk, and its crash model (`CrashPoint`) kills the
process at each save boundary.

**L3 — Publication recoverability.** What a device owes the server is a
function of durable state alone. **This is the law that was missing**, and
every entry in this file whose symptom is "an edit was dropped" is a violation
of it. The obligation lived only in `local_state.pending` — a queue of encoded
updates — and nothing anywhere recorded what the server holds in CRDT terms
(`DocumentSyncCursor` carries sequence numbers and an epoch, not a state
vector). So the obligation was not merely *forgotten* when that queue was lost,
it was **unrecoverable**, and the value it degraded to was "I owe nothing" —
the lossy direction.

**L4 — Replacement safety.** Every path that *replaces* durable state rather
than merging into it preserves what it discards. A CRDT merge is lossless by
construction, so every non-merge write is where loss can enter, and each one
owes a proof. The replacement sites are: epoch adoption
(`adopt_squashed_document`), account-switch reseed, crash recovery, the
loaders' error paths, and — until 0x — the integrity repair.

**L5 — Merge.** Yrs merge is commutative, associative and idempotent, and a
deletion survives merging with a state that does not know about it. Given.

**The composition.** L1 says what the user sees is what the device knows. L2
says what the device knows survives the process dying. L3 says the device can
always work out what to tell the server. L4 says nothing silently drops what
the device knows. L5 says once the server is told, every device agrees. Chain
them and an acknowledged intent reaches every device, which is the global
property.

Each law also says what a *violation* looks like, which is why attribution got
cheaper: an L1 break shows up as a field changing with no remote writer, an L3
break as a device that keeps something forever without publishing it, an L4
break as content vanishing at a specific sync step.

**Where they stand.** L1, L2 and L5 hold and are checked. L4 held everywhere
except the integrity repair, which 0x fixes. L3 is **partially** restored: 0x's
re-offer covers every document the server's integrity proof names, which is the
reachable majority. It is not a theorem, and the remaining hole is not a
missing line of code but a missing fact — see "Why Case A cannot be closed with
a cursor heuristic" and 0y.

### Why a full snapshot is safe where re-asserting the plain files is not

These look like the same operation and are not, and the difference is the whole
reason 0x could be fixed without reintroducing `offline_device_join`:

- A **full snapshot** republishes the structs the document already holds, under
  their original ids. Merging it into the server cannot resurrect anything the
  account tombstoned — the account's delete refers to exactly those struct ids
  and wins — and cannot erase anything the server has that this device lacks,
  because a merge only adds. It carries this device's tombstones, which is the
  point.
- **Re-asserting the plain workspace** (what `queue_local_only_documents_before_pull`
  does) writes plain items *into* the document, minting NEW structs with new
  ids that the account has never tombstoned. That is precisely how a fresh
  install's starter content comes back from the dead.

So "offer the whole document" is a safe default and "re-assert what the files
say" is not. The workspace index is excluded from the former anyway: its content
*is* the identity, and publishing a pre-join index over the account's costs the
account everything.

### Making L3 total

Persist, per document, the state vector the server is known to hold, **in the
same file as the document's bytes** so the two cannot be lost independently.
Then `owed(doc) = encode_diff(doc, acked_sv(doc))`, the pending queue becomes a
cache, and the degraded value of a missing `acked_sv` is the empty vector —
which yields a full snapshot, which by the argument above is lossless. That
flips the failure direction of every auxiliary-state loss from "sends too
little" to "sends too much", and too much is free.

The one discontinuity is the join boundary: before a device has joined an
account, its documents are its own and must not be published over the
account's; after, they must. That needs a durable witness outside the journal
(0y) — `workspace.sync.id` cannot serve, because
`canonicalize_personal_sync_identity` derives it from the account id at
sign-in, before any sync has happened. Chaos seed 11 is what a fix that skips
this step looks like in practice: a fresh install's starter tombstones
published onto the account's live rows.

### The coverage argument, by cases — and the case that stays open

A scheme document on a device can diverge from the server for any reason at
all — a lost journal, a dropped queue entry, a push the server did not keep.
Rather than enumerate causes, enumerate what the device can *know*:

- **Case A — no cursor for the document. STILL OPEN.** The device cannot
  compute a meaningful delta, and the server's integrity proof does not cover
  it either (its scope is documents whose cursor has advanced). Two attempts to
  re-offer anyway each cost other data, and both are recorded below because the
  second one is the real lesson. Test:
  `an_offline_deletion_survives_an_unmarked_journal_loss`, `#[ignore]`d and
  naming 0y.
- **Case B — a cursor exists.** The document is in the integrity proof's
  scope, and the proof runs on exactly the syncs that matter: it is gated on
  `local_state.pending.is_empty()`, so it fires precisely when the queue is not
  explaining the divergence. 0x makes that proof reconcile both ways. Test:
  `an_offline_deletion_survives_losing_only_the_outbound_queue`, which fails
  without it with the reported symptom.
- **Case C — the server has no base.** The existing bootstrap already sends a
  full snapshot.

The cases are exhaustive over "does this device have a cursor, and does the
server have a base". B and C are fixed; A is not, so the class is covered for
every document the server can name and open for the rest.

**Measured, not assumed.** Case B's test was run with its fix reverted and
fails with the reported symptom; the integrity path is exercised by the
in-process server in `shared/sync/src/testing.rs`, which mirrors the backend's
proof.

#### Why Case A cannot be closed with a cursor heuristic

Two attempts, both reverted, both caught by the fuzzer against a measured
baseline rather than by review:

1. **Re-offer a full snapshot of every cursor-less document.** Safe in the
   sense that matters — a snapshot merges into any base and cannot erase
   remote structs — but enormously loud: on a device that lost its journal it
   re-sends the entire workspace. Production fuzz seed **20082** went from
   passing to losing an archived folder on that change alone.
2. **Re-offer an exact `diff(local, server_state_vector)`** instead, the
   precise statement of what is owed. That fixed 20082 and then chaos seed
   **11** lost starter item `…2009` from scheme `…0102`. Both ids are
   *derived*, so they are byte-identical on every account and every fresh
   install: the device was publishing its own **pre-join starter tombstones**
   onto rows the account holds live, which is `starter_content_join.rs` /
   `offline_device_join.rs` reached by a new route.

**The lesson is the overloading.** "This device has no cursor for the
document" means one of three things that need opposite handling:

| State | Correct behaviour | Distinguishable? |
|---|---|---|
| First join | re-assert nothing | **no** |
| Lost journal | re-assert everything | **no** |
| Account switch | defer to the switch reseed | yes (`needs_full_reseed`) |

A first join and a lost journal are indistinguishable from durable state today,
because `canonicalize_personal_sync_identity` derives `workspace.sync.id`
straight from the account id **at sign-in**, before any sync has happened. A
device that has synced for months and lost its journal presents exactly as one
that just signed in. Any rule keyed on cursors therefore has to guess, and
guessing wrong in either direction destroys data belonging to somebody.

That is what 0y is for, and why it is a prerequisite rather than a nicety.

### The hazard is now in the sweep, not just in a repro

`desktop_production_journal_loss_fuzz` runs the everyday single-account
configuration with one addition: a device periodically loses `sync-state.json`
between quit and launch, with the CRDT documents left intact. It is checked by
the same no-silent-loss oracle as every other step — deliberately *not* treated
as a modeled-loss boundary the way a crash is, because losing bookkeeping about
what was sent must cost nothing. `journal_loss` is off in every other
configuration and the roll values it claims fall through to the identical local
action when off, so no catalogued seed's trajectory moves.

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

**Measured on `c5164b0` at 400 x 300, one seed per process: 5 failing seeds of
800** — `10054`, `10117` (single-account) and `194`, `332`, `389` (chaos). That is
down from the 7 first measured here; `10209` and `10350` are fixed. The unmerged
`spike/landing-placement-reconcile-gate` would take it to 4 (`10117`, `332`, plus
`48` and `238` surfaced), which is why the count quoted for the spike differs.

**The original 7, for the record.** The
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

**Root cause found 2026-09-30: `reassert_local_scheme_edits` publishes nothing.**
Measured, not inferred — the seed replays in about three seconds, so each of these
is a direct observation:

- Disabling `reassert_local_scheme_edits` alone makes 10175 **pass**. Nothing
  else changed.
- Exactly two scheme reassertions happen in the whole run, both on **device 0**:
  `SetSchemeGsync { 2b1646c2…, on: true }` during an in-flight landing at step
  100, and `SetSchemeGsync { ba929b98…, on: false }` at step 200. The first is one
  of the diverged fields — `2b1646c2…`'s `SchemeSource` reads
  `[{"kind":"local"},true]` on device 0 against `false` on device 3.
- Bypassing the landing's merge path — forcing `adopt_sync_workspace` to take
  `replace_workspace_from_sync_result` even when local edits happened in the
  in-flight window — **also makes 10175 pass**. So the merge preserving a local
  index-field value is part of the same story.
- The run reports **no projection divergence at all**, so device 0's view and its
  own documents agree throughout. It is the *account* they end up disagreeing
  with: device 0 settles on `true`, the account and device 3 on `false`, and no
  device is left wedged.

**A wrong turn worth recording, so nobody repeats it.** The first reading here
was that the reassert "publishes nothing": summing `crdt_updates` across pending
operations gives a delta of 0 across both reassertions. That is a measurement
artifact — the store defers CRDT encoding, so `crdt_updates` is empty until a
flush. `unsynced_edit_count`, which counts deferred work, goes 1→2 and 0→1 across
the same two calls. The reassert's decision *is* queued for push.

Which means causation is still open. Both switches above change the outcome, but
neither has been shown to be the thing that is *wrong*, and "disabling X makes the
seed pass" is exactly the evidence that misled the 48/238 write-up below: a change
can perturb the trajectory enough for the oracle to observe a transition it
otherwise steps over. Note that two of the three diverged fields — `70db3d43…`'s
name and colour — are touched by no reassertion at all, which is what a trajectory
effect would look like.

**Where the divergence actually lives, measured at the field level.** Device 3 is
the stale side, and the reassertions both run on device 0, so they cannot be what
makes device 3 stale. Instrumenting the per-field merge in `workspace_index.rs`
for `70db3d43…` and correlating each reading with the step log:

- Immediately after device 3's last sync, its own `node_fields` still hold
  `("color_index", "9")`, `("name", "scheme 2379")` and `("position", …)` from
  creation, while the account's readings hold `16`, `"renamed 4275"` and
  `position 5`. Both sides are `field_schema=Some(1)`, so the legacy-payload
  escape hatch is not involved.
- Device 3 created that scheme at step 37, before its own first sync at step 57.
  Devices 2 and 1 changed the colour at step 71 and the name at step 137.
- Device 3 ends with an empty pending queue, so those creation-time writes are
  writes to the same Yjs map keys that the account never received and that win
  locally. Not a read-side shadow: an unpublished write.

**Two more hypotheses ruled out.** Neither of these is the cause:

- *Not* `field_schema` being absent on device 3's entry — it is `Some(1)` on both
  sides, so the "older build wrote this, keep its payload" branch never fires.
- *Not* the population base being lost by the relaunch device 3 performs at step
  41, before its first sync. That looked compelling — every capture site is gated
  on `workspace_document_is_unpopulated()`, so a relaunch after the first save
  cannot re-capture, and `reidentify_workspace_document`'s own comment says it
  "only rebinds the document without touching the wrong-hashed population
  inside". But instrumenting `workspace_population_base` shows it is
  `base_present=true` at **every** canonicalization in the run, device 3's
  included. The repopulate branch is being taken, not the plain re-key.

**Root cause, measured.** `repopulate_workspace_canonically` rebuilds the document,
and whether that rebuild ever reaches the account depends *only* on
`index_changed` — the comment beside it says why: there is no "before" left inside
the freshly built document, so the incremental flush finds no delta and the
explicit full-snapshot queue is the only publication path.

`index_changed` is `workspace_document_differs(&canonical_base, &workspace)` —
whether this device edited **on top of its own pre-sync base**. Content that was
already in the base, such as a scheme the device created before its first sync,
makes it `false`. Instrumenting the queue at device 3's canonicalization (step
195) prints `index_changed=false full_updates=1 -> NOT PUBLISHED`: a usable
snapshot is discarded. Device 3's `node_fields` writes then stay local forever,
win against the account's later values on that device alone, and leave an empty
pending queue, so no wedge or projection check can see it.

**The obvious fix is wrong, measured at release depth.** Publishing whenever the
repopulated document has state (dropping the `index_changed` gate) makes 10175
pass and **breaks two other seeds**: chaos 148 with three violations, and
single-account 10192. Like-for-like on the same machine, same command, same
depth:

| | 300 x 200, `production_fuzz` |
|---|---|
| unmodified | 30 passed, 1 failed (10175 only) |
| publish always | 29 passed, 2 failed (148, 10192) |

That local baseline matters: it is identical to CI's, which is worth knowing given
10175 itself fails on macOS at the v0.57.0 tag while CI's Linux run passed it. The
comparison here is macOS against macOS.

Why it breaks them is presumably the thing `index_changed` was guarding — a device
that has not seen the account's deletions publishing a full snapshot can
reintroduce what the account dropped, which is the shape of `0a`. So the two
comments in that function are not simply contradictory: a full snapshot does merge
rather than replace, and merging is exactly how deleted content comes back.

**Next approach, untried:** the mirror image. When `index_changed == false` there
is no local index edit worth keeping, so instead of repopulating from this
device's base — which is what leaves the unpublishable writes — adopt the
account's document wholesale for the index. Bypassing the merge path entirely
(forcing `replace_workspace_from_sync_result`) already makes 10175 pass, which is
the same effect reached with a blunter instrument, so the direction has some
support. It is resurrection-safe by construction, because nothing local is sent.

**...and that next approach is wrong too, by inspection.** `index_changed == false`
does not mean "this device has nothing the account needs". It means "this device
made no edit *on top of* its base" — and the base itself can be the offline
content that has to reach the account. A fresh install that created schemes before
its first sync has `index_changed == false` and still must publish. Adopting the
account's index wholesale there would drop exactly what these currently-passing
scenarios exist to protect: `new_install_with_offline_edits_joins_the_account`,
`join_variant_empty_workspace_with_offline_edits`,
`join_variant_starter_already_on_the_account_identity_with_offline_edits` and
`starter_lines_edited_before_the_first_sync_join_the_account_once`. Do not run it
expecting a green sweep.

What separates 10175 from those cases is not a document-level property at all. In
10175 the account **already holds the node entry**, with newer field values, and
the device's writes are stale duplicates of keys the account has moved on from. In
the join variants the account holds nothing for those entries. So the decision is
per key, not per document: publish the device's index writes for entries the
account does not have, and let the account win for entries it does.

That is what the `node_fields` per-field merge would already do if the device's
pre-canonical writes were *comparable* to the account's — they are not, because
they were authored under an identity the account never saw, so they are concurrent
and win locally by clientID. Which lands back on TODO 1's deterministic population
identity: the repopulation has to be authored so that it loses to the account's
real writes on any key the account already holds. That is the fix that has been
attempted twice and reverted twice, and the measurements above are the sharpest
statement so far of *why* it is needed — not a new, smaller alternative to it.

**Per-key filtering does not rescue it either, and the reason names what is
missing.** The tempting narrow version is: when repopulating, omit the keys the
account's incoming state already holds, so the account wins there, and publish only
the keys it lacks. Two dead ends:

- Moving the *publish gate* to "does this device hold index content the account
  lacks" (comparing against `sync_workspace` instead of `canonical_base`) makes
  `index_changed` true for any device that has not yet merged the account, which is
  publish-always — already measured above as 29/2.
- Filtering inside `repopulate_canonically` instead loses real work. A device that
  renamed an *existing* account scheme while offline, before its first sync, has
  written a key the account also holds; dropping it to let the account win discards
  that rename. Content loss, not divergence.

Separating those two cases needs per-field provenance — "did this device *change*
this field offline" versus "did it merely carry the field out of its own
population" — and a pre-sync device records nothing of the kind. Which is exactly
the gap a deterministic, content-derived population identity closes: it makes the
population itself recognisable, so a real edit on top of it is distinguishable from
the population's own writes. There is no shortcut around that property.

**Where the polluted base comes from, measured.** At device 3's canonicalization:

    base:    schemes=6 70db=["scheme 2379/9"]
    current: schemes=6 70db=["scheme 2379/9"]   index_changed=false

The base already holds the scheme device 3 created, so the comparison is content
against itself. Instrumenting both re-capture sites shows neither fires — the base
comes from `WorkspaceStore::new`, which captures it whenever the index document is
unpopulated. After a **relaunch** that is the entire on-disk workspace, already
containing content this device never published.

So `workspace_population_base` does double duty and the two uses conflict: it is
the population source for the rebuild (must hold the content) and the change
detector for `index_changed` (would need to be the pre-edit state). A relaunch
before the first sync is where they collide.

**Three fixes tried, all rejected by the gate at 300 x 200.** Baseline on this
machine is 30 passed / 1 failed with 10175 the only failing seed:

| change | result |
|---|---|
| publish whenever the rebuilt document has state | 29/2 — chaos 148, single 10192 |
| `index_changed` compares against the account instead of the device's base | 30/1 but **two** failing seeds: 10106, 10192 |
| capture the base after `clear_pushed_edits` | 10175 unchanged |

The second is the interesting one: it is not publish-always — it keeps a real gate,
just pointed at "does this differ from the account" — and it still over-publishes.
With the first, that **brackets the problem from both sides**: any *document-level*
test either under-publishes (10175 keeps its stale fields) or over-publishes
(10106/10192/148 start diverging). Which is where the per-key analysis above
arrived from the other direction, now with measurements behind it. The distinction
that has to be made is per-field — did this device *change* this field, or merely
carry it out of its own population — and that is the provenance a pre-sync device
does not record.

**A fourth attempt, and the structural reason all four fail.** Tried: replace the
content diff with a flag — "has an index-touching command been applied since the
base was captured" — on the theory that the diff was only ever standing in for
that question, and is confounded because the base doubles as the population
source. 10175 still fails, and the trace says why: device 3 relaunches *again* at
step 155, so the store is rebuilt and the flag starts false, and between 155 and
its canonicalization at 195 it issues only item commands (`open a day`, `move line
to scheme`, `marker`, `set date`) — never an index-touching one. Nothing triggers
publication.

That is the structural point behind all four falsified attempts. **The unpublished
index content predates the current store instance.** A relaunch rebuilds from
disk, where the plain workspace holds it and the index document does not, and
nothing available *within that session* can distinguish "content this device owes
the account" from "content the account already has". Not a content diff against
the base (the base contains it), not a diff against the account (over-publishes:
10106, 10192), not an edited-since flag (no edit follows), not publishing
unconditionally (resurrects what the account deleted: 148, 10192).

The missing information is on disk, not in the session: which of this device's
index writes have ever been published. That is the provenance a deterministic,
content-derived population identity encodes, and it is why TODO 1 keeps being the
answer no matter which direction this is approached from.

**One fix tried and rejected:** capturing the scheme edits *after*
`clear_pushed_edits` rather than before, so only unpushed operations are
reasserted. 10175 still fails — which also rules out the "already acknowledged,
account resolved otherwise" reading, because the operation driving the reassert
is genuinely unpushed.

That leaves the real question: when a device's own document holds a field value
the account resolved against, what is supposed to bring the account's value into
that document? The reassert sits downstream of that gap rather than causing it,
and `adopt_sync_workspace`'s merge path is where to look. Do not "fix" this by
making the reassert write harder — publishing an unpublished local decision would
drag the account onto a value the CRDT had already resolved away, which is the
second-conflict-resolver mistake recorded elsewhere in this file.


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

### A second nightly step was running zero tests

Found 2026-10-02 while running the nightly's own commands by hand. Its
multi-origin daily-queue step passed `-- --ignored --exact
daily_queue_multiorigin_stress`, and `--ignored` means *run only ignored tests*.
The test was un-ignored when it became fast enough for every `cargo test`, so
from that moment the step reported success having run nothing:

    running 0 tests
    test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 19 filtered out

Same shape as the link failure above — a nightly step that looked green and was
not doing its job — and the second one found in this file. Fixed by
`--include-ignored`, which runs the test whether or not it is ignored, so
re-ignoring it later cannot empty the step again. The coverage was not entirely
lost in the meantime: the test is in `sync_property_model`, so the deep step
above ran it at that step's `KNOTQ_FUZZ_SEEDS`, just never at the 3000 this step
asks for.

**The lesson, again: run the gate's own command lines and read how many tests
they report.** Both holes were invisible to anyone reading the YAML.

### Re-measured 2026-10-02, after 0A

At the nightly's own depth (`KNOTQ_FUZZ_SEEDS=400 KNOTQ_FUZZ_STEPS=300`,
one seed per process), of the five seeds the sweep still blocks on:

| Seed | Before | After 0A | Note |
|---|---|---|---|
| 332 | fail | **pass** | the resurrection in 0A was its whole mechanism |
| 389 | fail | **pass** | passed before 0A too, on the current tree |
| 10117 | fail | **pass** | likewise; the doubled-text gap it pinned is still real, see its section |
| 194 | fail | fail | a lost Daily Queue *binding*, not a lost item |
| 10054 | fail | fail | traced below; a different root cause from 0A |

Running the nightly's own four commands at its own depths on that tree gives:

| Nightly step | Result |
|---|---|
| `knotq-sync` full suite | green |
| production_fuzz 400 x 300 | **red**: 194 (chaos), 10054 + 10350 (single), 20223 (journal-loss) |
| multi-origin daily-queue, 3000 seeds | ran **zero tests** — see the section above |
| `sync_property_model` 1500 x 400 | green, 15 passed, 2596 s |

Two of those four are newly named and both were attributed before being believed:

- **20223** (journal-loss, loses a Daily scheme + its item + its binding at step
  33) fails **identically with the 0A rule disabled**, so it is not a regression
  from it. That sweep had never run at 400 x 300 before, so this is depth, not
  change — and it is the same Daily-page family as 194.
- **10350** is not attributable to a seed at all: see "The sweep's answer depends
  on the PROCESS". It passes 40/40 as its own process and passes in an isolated
  sweep at one and at four workers. Note that its symptom (step 202, item
  `92b72501…`, scheme `ba929b98…`) is the one this file records as FIXED on
  2026-09-24, so the obvious reading — "that fix regressed" — is wrong.

A second run of the same sweep on the same tree then named **10374** instead, and
that one is real: step 248, device 3 loses schemes `016116db…` "scheme 3485" and
`11d75484…` "scheme 2540" plus folder `6db57f8c…`, four violations, failing
identically with the 0A rule disabled. It is a scheme/folder loss — the
workspace-index class, not the Daily-page class the other three belong to — and
it is **newly named here**, having been missed by the run that should have found
it. Treat the list of red seeds in this file as a lower bound.

389 and 10117 passing is a trajectory change, not evidence that the defects they
exposed are gone — 10117's `concurrent_same_item_creation_with_different_content_doubles_text`
still fails as a unit test. Treat them as "no longer selected by this corpus".

**10054, traced 2026-10-02.** Not a resurrection and not the placement gate:

- step 239: device 1 moves item `88b70256…` out of Daily 09-17 (`6eff952f`) into
  scheme `5f4b96ab`.
- a later step permanently deletes the FOLDER holding `5f4b96ab`
  (`PermanentlyDeleteFolder { 8d6870c9 } => -folder 8d6870c9 -scheme 5f4b96ab`),
  so the destination scheme is destroyed and the copy in it goes with it.
- step 270: device 3, which never saw the move, carries the row from 09-17 to
  Daily 09-18 (`244425d7`) — a live copy again.
- step 274: `sync: the pull dropped 1 scheme(s) this device held: 5f4b96ab
  (published=true unpushed=true …)`, and the server view holds the row nowhere.
  Daily 09-18 is reported as `materialized no scheme: 069440c6 doc=244425d7
  live=false deferred=true` — **the account's index has the binding and no node
  entry**, so a fresh joiner cannot materialize that day at all.

Two separate things to fix, neither of them 0A:

1. `Attribution::moved_into` keeps only the LAST destination of a move
   (`HashMap::insert`), so `moved_into_a_destroyed_scheme` stops excusing the row
   once a second move overwrites the entry. Here the first destination *was*
   destroyed. A `HashMap<ItemId, HashSet<SchemeId>>` would be the faithful shape.
2. The real defect is the one `materialized_workspace_repair` already names in a
   comment: *"a day should not reach the index with a binding and no node entry"*.
   `workspace_index.rs` retains an unloaded scheme's node entry only
   `if let Some(stored) = stored_nodes.get(&id)`, so a device whose own index
   document has no entry for it contributes none — and `sync_string_map` then
   removes it for the whole account. Rebuilding the page in `materialize` is
   explicitly the wrong fix (chaos 108 / single-account 10214); synthesizing the
   missing *node entry* at the index write is a different and untried move.

**194** is the same family seen from the other side: the binding itself goes
missing at settle.

**Decision still to make.** The link fix alone turns the nightly from "red
because it cannot build" into "red because it finds real seeds". Either
drain them and keep 400 x 300, or bring the file down to a depth that is
actually green and raise it deliberately afterwards — which is what the note
above ("raising the gate's depth is worth doing only once the sweep is green")
already says, written while this file quietly specified a higher one.


## 0x. [FIXED] The integrity repair resolved every disagreement in the server's favour, and deleted unsent local work to do it

**Reported from the field 2026-10-01**, and the first entry here that came from a
user rather than a fuzzer: a line deleted while the app showed "offline" was back
on every device after the next sync. Consistent, so not a convergence failure —
the deletion was simply dropped.

**Reproduced** by `shared/sync/tests/offline_deletion_durability.rs`, which states
the property the report violates — *a deletion made while a device cannot reach
the server survives whatever happens before its next successful sync* — and walks
the interruptions a real offline period can contain: a restart, repeated failed
syncs, a push reseed, a concurrent edit elsewhere, a damaged journal, an unmarked
journal loss, losing one scheme's CRDT state, an abandoned unlanded pull, and
emptying a scheme completely. Nine of the ten passed. The tenth — the journal
gone with nothing marking it — lost the deletion, exactly as reported.

**Root cause: two separate things, both in the integrity-repair path.**

The server periodically proves document hashes to the client. On a mismatch the
client reset the cursor, re-fetched full state, and then force-**adopted** it:

```rust
let needs_adoption = |doc: &PulledCrdtDocument| {
    doc.kind == SyncDocumentKind::Scheme
        && (integrity_repair_documents.contains(&doc.document)   // <- wrong bucket
            || cursor.epoch != doc.epoch)                        // genuine squash
};
```

Adoption *replaces* the local document. That is correct for an epoch squash,
where the server's state shares no Yjs history and merging would double every
item's text. It is wrong for an integrity mismatch, where the epoch is unchanged
and the two states still share history — a merge there is exact.

The only thing standing between a user's unsent work and that replacement was
`!local_state.has_pending_for_document(document)`. The pending queue is therefore
load-bearing for data safety, which it is not durable enough to be: it lives in
`sync-state.json`, the file that is lost or defaulted in every scenario this
document already catalogues. `adopt_squashed_document` even names the hazard in a
comment — *"this is the one place an adoption can cost content"* — and only logs
it, and the log only covers items the local document **has**, never ones it has
**deleted**, which is the reported case.

**The second half is that the repair was one-directional.** Merging fixes this
device's copy of the disagreement. Nothing fixed the server's: with no pending
entry, the device kept its deletion and could never tell anyone about it. So
even with the merge in place the account stayed wrong — loss turned into a
permanent stall.

**The fix** is to treat an integrity mismatch as what the server actually said —
these two copies disagree — and resolve it symmetrically:

- Integrity-mismatched documents **merge** instead of being replaced. Epoch
  adoption is untouched and still replaces, which is correct for a squash.
- After the merge, the mismatched documents are **re-offered to the server as
  full snapshots**. A snapshot merges into any base, so it can only add; it is
  scoped to the documents the server named, so it never becomes a workspace-wide
  reseed; and it needs no pending queue, which is exactly what is missing.

The result is self-correcting: the device ends up a superset of the server, the
server merges the snapshot, and the next proof agrees.

**A wrong turn worth recording.** The first attempt inferred "my journal was
lost" from *no cursor history + the account already knows my workspace document*
and set the existing `storage_recovery_pending` flag. It is wrong, and
`cross_version_compat::a_document_epoch_from_the_future_does_not_panic` caught
it: `canonicalize_personal_sync_identity` derives `workspace.sync.id` straight
from the account id **at sign-in**, before any sync, so a genuine first join is
indistinguishable from a lost journal by that test. Re-asserting there is the
`offline_device_join.rs` disaster — the joining device writes its own index over
the account's. Telling those two states apart needs a witness that survives the
journal and is written only after a run completes; there is none today (see 0y).
The fix above avoids needing one, because the server names the documents.

## 0A. [FIXED] The post-pull repair re-expressed rows the pull had just removed

**Chaos 332 and the placement ping-pong family are this one defect.** Found
2026-10-02 by tracing presence tags, not by reasoning about the landing.

`local_ahead_items` is computed **before** the pull, and the post-pull repair
re-expresses it afterwards. A row can be in that set for two very different
reasons, and only the pre-pull comparison can still tell them apart:

- this device's document held the row **live with a different value** — a real
  local edit that has not been flushed. Edit versus a concurrent remote delete
  resolves toward keeping the content, deliberately;
  `persistence_boundary_fuzz_converges` pins that and its comment says so ("the
  local edit must be re-expressed after the remote tombstone is merged rather
  than disappearing").
- this device's document had **no entry for the row at all**. Then the only
  reason it is in the set is that the plain copy lists it — and the plain files
  are a projection of the documents, which lags every pull. That is no evidence
  of an edit, and if the pull now carries a removal for the id, the account knows
  that row and has deleted or moved it.

The code re-expressed both, so the second case resurrects. The same function
already refused the mirror inference in the other direction, and had for a long
time: *"Lines the CRDT has and the plain copy does not are NOT treated as local
deletions. 'Plain lacks it' is ambiguous."* The converse was never stated.

**The mechanism, measured step by step on 332.** The daily documents are
`ea4f07d4` for 2026-09-14 and `f9bc2620` for 09-15:

| step | what happens |
|---|---|
| 9 | device 1 carries line `…0402` from 09-14 to 09-15: a tombstone in `ea4f07d4`, a `seed:` presence tag in `f9bc2620` |
| 45 | **device 0 pulls that tombstone and the post-pull repair puts the row back on 09-14** — a `resurrect:` tag in `ea4f07d4`. Device 0's 09-14 document held no entry for the row: `crdt_live=0 tombstoned=0 plain=2` |
| 53 | device 0, which now sees the row on 09-14, moves it to 09-15: a fresh entry in `f9bc2620` |
| 55, 56 | devices 1 and 3, which see the row on 09-14, tombstone the 09-15 copy |
| 60 | device 0's landing removes its own 09-14 resurrection |
| 70 | the row is live in neither day. The oracle reports it, correctly, as an item no device deleted |

The write itself is `ensure_item_presence`'s `resurrection_epoch` arm, reached
because `replace_scheme_inner` sees a stored entry that is `deleted` while the
scheme it is told to write still lists the row. `replace_scheme` is right to have
that path — an undo, a paste or a retype does re-add a row over a tombstone — but
a repair is not an edit. It runs precisely because the two halves already
disagree, which is the one situation where the plain copy's claim carries no
weight on its own.

**Three measurements that each killed an earlier version of this fix. Read them
before changing this code again.**

- It is the **post-pull** repair, not the pre-pull one. A pre-pull-only rule
  leaves 332 failing, because pre-pull there is no tombstone yet: the pull is
  what delivers it.
- "Tombstoned" cannot be `deleted_item_ids`, which filters out entries with an
  empty snapshot. 332's entry is exactly that shape — the removal arrived before
  content was ever populated locally — so the first attempt changed nothing at
  all.
- **Comparing content does not work**, and this is the one that is genuinely
  counter-intuitive. The obvious rule is "re-express it only if the plain row
  differs from the tombstoned row, i.e. somebody edited it". Measured on
  `persistence_boundary_fuzz_converges`' own seeds, the two are **equal**:

      local_text=Some("plain-repair-2654435769-0") doc_text=Some("plain-repair-2654435769-0")

  because the pre-pull repair has already written the local edit into the
  document, and the pull then merged the peer's tombstone on top of it. By the
  time the post-pull repair runs, an edited row and a stale row look identical.
  The distinction has to be captured at the pre-pull comparison, where the
  document either had a differing live entry or had nothing — hence
  `local_absent_items`.

Dropping an id from `ahead` would not have been enough either: `ahead` only
steers `merge_items_for_adoption`, which only runs when the scheme already had
raw-only items. The rule drops the id from `touched` **and** from the repair
input, so `merge_items_for_adoption` leaves it out of the merged scheme and
`replace_scheme` never sees it.

**Also in this change:** `scheme_entry_summary` answers "live rows", "replayable
tombstones" and "every removal" from ONE decode of a scheme document. The
per-scheme comparison every pull runs was already paying for two full decodes
(`raw_scheme_items` plus `raw_scheme_deleted_item_ids`), so adding the third
question made it cheaper rather than more expensive.

**Pinned by** `a_remote_deletion_is_not_undone_by_a_device_whose_plain_copy_is_ahead`
and `a_local_addition_still_lands_when_the_same_scheme_holds_a_remote_deletion`
(`shared/sync/tests/offline_deletion_durability.rs`), both of which fail without
the change. The second is the guard that matters: the easy way to "fix" this is
to stop re-expressing anything, and a line the user really did type in that same
scheme must still reach the account.

### The re-offer needs a termination bound, and only the WebSocket suite found it

Added 2026-10-03, after the fix above was already measured green by the whole
per-seed census. The integrity re-offer is a full snapshot per named document, and
nothing stopped it repeating: the device re-offers, the server's next proof still
disagrees, so it re-offers again. Against a real backend
(`run-sync-stress.sh --fuzz`, which no amount of in-memory fuzzing substitutes for)
`ws_account_hopping_fuzz_converges` livelocks on it — "re-offering 11 document(s)"
without end, the server answering `rate_limit.exceeded`, and finally "a device on
account 0 has stuck pending (wedge)". `origin/main` passes that suite, so it was a
regression this work introduced, and a WEDGE at that.

Three bounds measured, in order:

| Bound | `ws_account_hopping` | census |
|---|---|---|
| skip documents that already have a queued edit | still wedges (11 → 6) | — |
| drop the re-offer entirely | green | **chaos 34 and 66 lose content**, 4 `offline_deletion_durability` cases fail |
| **once per document per mismatch episode** (`integrity_reoffered`, cleared when the server reports nothing mismatched) | **green** | **clean** |

Only the third terminates *by construction* rather than by hope. "Only when the
queue is empty" sounds like it should be enough — and it is the condition that
makes the repair useful, since a lost journal is exactly an empty queue — but the
device drains, re-offers, disagrees again, and round it goes; measured, it still
wedges.

**The lesson for the gate: the in-memory census and the real-transport suite find
different classes.** A change can be green across 1200 seeds and still wedge a
device against wrangler, because what livelocks is the interaction with a server
that rate-limits and recomputes proofs. Run both before claiming a sync change is
safe.

## 0B. [FIXED] The re-identification rescue replaced a real workspace index with an empty document

Found 2026-10-03 by tracing journal-loss seed 20223, the only journal-loss seed
the 2026-10-02 census failed.

**Census, both trees measured back-to-back with a verified harness binary and the
same instrument** (the first three runs of it were measuring nothing — see the
census section's warning):

| Configuration | `origin/main` | this tree |
|---|---|---|
| chaos (1–400) | **194, 332** | **38** |
| single-account (10000–10399) | **10054, 10117** | clean |
| journal-loss (20000–20399) | **20223** | clean |
| | **5 failing** | **1 failing** |

194 / 10054 are the cross-scheme-move family, 332 / 10117 the post-pull
resurrection, 20223 this entry — the four fixes in this body of work, each
confirmed red on main and green here. The one that remains, chaos 38, is a
pre-existing defect this work newly *selects* rather than causes; it is diagnosed
in full in 0D.

Single-account **10395** (`sync left item … in the workspace more than once`, the
placement family) also deserves recording: it failed 10/10 replays in one window
on BOTH trees and 0/12 in another, with no code change in between. So a seed's
outcome is not stable over time on one binary, which is the missing half of "The
sweep's answer depends on the PROCESS" below — that section bounded the variation
to "something shared inside a process" and this rules even that out. Until the
cause is found, read every seed list in this file, including the table above, as a
sample rather than a verdict.

The fix is one guard, and the two-line class of mistake behind it is worth
stating on its own: **carrying content is always an improvement; carrying
ABSENCE is a deletion.**

### What the fuzzer reported, and what 0w got wrong about it

    sync: pre-pull local-only repair skipped: workspace document not seeded yet
      (doc=051f6cea… state_bytes=2 cursors=6 schemes=6)
    sync: the pull dropped 1 scheme(s) this device held: 1ec12563…
      (published=false unpushed=true archived=false daily=true)

0w read that 2-byte index on a device holding 6 cursors as a **deadlock** — "a
device whose own index is empty while the account's is also empty can never
publish its index". That is not what happens. The account's index was 12 KB and
**on this device's disk the whole time**, under the canonical document id. The
device had simply stopped looking there, and the sync run then overwrote it.

Measured, in order:

1. Device 2 installs with `workspace.id` 9dbe442b and `sync.id` b24e741c —
   `Workspace::new` draws the two independently — and signs in to account
   051f6cea.
2. Its first sync lands through `replace_from_sync`, which replays unpushed
   pre-sign-in edits over the account's document. One is the index *population*,
   which carries `meta.id`/`meta.sync`; Yjs resolves a map key by last writer, so
   the workspace materialized afterwards wears the PRE-SIGN-IN identity.
   `reroot_pre_sign_in_edits` canonicalizes to `self.workspace.id` — read back
   after that replay — and `sync.id = DocumentId(workspace_id.0)` therefore
   derives an index document id **from a local `WorkspaceId`**. The saved
   `workspace.json` names 9dbe442b for both.
3. `from_states` keys the index by `workspace.sync.id`, so every later launch
   builds it EMPTY while `051f6cea.ydoc` sits on disk with the real index.
4. The next sync canonicalizes back to 051f6cea, sees the document id change as
   an account switch, and the re-identification rescue carries the previous id's
   state across. It chose its source by **presence**:

   ```rust
   if crdt_states.contains_key(&previous_workspace_document_id) { … }
   ```

   An unwritten Yjs document is the canonical two-byte update `[0, 0]`, not
   zero-length, so the stale id *was* present — and the rescue removed that empty
   state and inserted it under the canonical id, replacing a 12 KB index with
   nothing, then queued the nothing for push.
5. `from_states` built the index unseeded, so
   `queue_local_only_documents_before_pull` published nothing, and the pull
   materialized the account's index over device 2's local-only Daily page. The
   page, its line and its queue binding went together.

### The fix, and why it is deliberately this narrow

`sync_snapshot_in` now refuses exactly one move: carrying a source with no
operations onto a canonical id that has some. Nothing else changes — the source
selection above it is untouched. `knotq_sync::crdt_state_is_empty` is
`update_v1_is_empty` made public, because `is_empty()` is the wrong test for a
Yjs state and was the wrong test here.

**Widening it was tried first and cost eight seeds.** Choosing the source by
"whichever id carries content", and reading a present-but-empty canonical state
as absent for the by-shape fallback, both look like the same idea and move a
first sign-in onto a different path: chaos **132, 178, 289, 295, 320**,
single-account **10156** and journal-loss **20047, 20144** all went from green to
failing, every one of them green on `origin/main`. Narrowing the guard to the
destructive case alone returned seven of the eight; the eighth was the other half
of the attempt, below.

### A real defect found on the way, and the fix for it is FALSIFIED

Step 2 above is its own bug: a signed-in device saves a `workspace.json` naming
an index document that exists nowhere, and keeps it until its next sync. A device
that never syncs again keeps it forever.

Passing the account's canonical id into `reroot_pre_sign_in_edits` (captured
before the replay) does fix the identity, and **wedges chaos 178**: it makes that
call re-key `id`/`sync.id`, which queues index edits *there*, after
`remap_pending_workspace_document` has already run. Device 0 ends with five
unpushed `PersonalWorkspace` edits addressed to the stray document and never
drains its queue ("still has 5 unpushed edit(s) after settling"). It also moved
20223's own failure rather than removing it — to step 163, where the account loses
a row across a cross-scheme move (the placement family, §1 of "The two modeling
choices"). With the re-root left alone and only the guard in place, 20223 is green
at 300 steps.

Re-keying the plain workspace is half of the job; the document's content and the
edits addressed to it have to move with it, which is what
`adopt_sync_workspace_identity` already does on the merge path. Pinned as an
`#[ignore]`d test in
`desktop/state/tests/sign_in_keeps_the_account_identity.rs`. **Do not re-apply
the one-liner.**

### Pinned by

`a_relaunch_does_not_come_back_with_an_empty_workspace_index` (production_fuzz)
replays 20223 in `replay_journal_loss_seed`'s exact environment — squash
thresholds forced on, maintenance steps off — at 200 steps, because at 120 the
ablation is not selected.

## 0D. [DIAGNOSED, NOT FIXED] A failed first sync lets a fresh install delete the account's folders

Found 2026-10-03, by chaos seed 38, and **this one is a field-plausible
data-loss path rather than a fuzz curiosity**: all it needs is a new device
signing in on a flaky network.

What the trace shows, with device 5 installed and signed into account 0 at the
last step of the run:

    device 5 installed, account Some(0)
      pre-pull repair: first sync: index repair suppressed, authored lines only
    device 5 run: failed: memory server: connection dropped
      pre-pull repair: repairing 0 missing scheme doc(s), index_mismatch=true,
                       5 scheme(s) with local-ahead content
    sync: workspace index write removes 3 node entr(ies): 38de9f2d…, 5f44979a…, eff50a3d…
    device 5 run: pushed 11 doc(s) …
    VIOLATION: device 5's sync (server state) lost folder eff50a3d… that no device deleted

The first run is protected. The second is not, and it publishes an index built
from this device's **pre-sign-in** plain workspace; `sync_string_map` means "the
account's index is now exactly this", so three folders the device had not yet
pulled became a deletion for every device on the account.

### The cause is one predicate asking the wrong question

```rust
let first_sync_with_this_server = local_state.document_cursors.is_empty();
```

A run that fails partway still leaves cursors behind, so "has a cursor" and "has
ever synced successfully with this server" are not the same thing — and the gap
between them is exactly where the suppression is load-bearing. This is the same
overloading of "no cursor" that the `WHAT IS STILL MISSING` note in
`batch_pull_and_apply` describes from the other direction, and the witness 0y
wanted. **It does not need a new witness.** The workspace document's own pull
cursor already records it: `last_pulled_sequence > 0` means the server has
actually sent this device the account's index.

```rust
let has_pulled_the_accounts_index = local_state
    .document_cursors
    .get(&crdt_docs.workspace_document_id())
    .is_some_and(|cursor| cursor.last_pulled_sequence > 0);
let first_sync_with_this_server =
    local_state.document_cursors.is_empty() || !has_pulled_the_accounts_index;
```

A device that has synced for months is unaffected — its cursor is long past zero.
A device that has never received an index keeps the conservative path, where its
content still reaches the account through the post-pull bootstrap's full
snapshot, which can only add.

### …and that fix was measured, and it is WORSE. It is not in the tree.

It does what it says: chaos **38** goes green and chaos **253** — the seed this
file records as needing the repair to actually RUN — stays green. And the full
1200-seed census goes from **one** failing seed to **three**:

| Seed | What it becomes |
|---|---|
| chaos 109 | **wedges** — `device 4 still has 1 unpushed edit(s) after settling`, a `PersonalWorkspace` edit that never drains |
| chaos 142 | `a fresh device is missing content existing devices have (server lost it)` |
| chaos 269 | the server's view loses an item at step 249 |

All three are green on `origin/main`. The wedge is the same shape as "Attempt A"
under §2, and the reason is the same: **suppressing the index repair for longer is
not free.** The repair is what publishes a device's own index content, so withheld,
the queue behind it has nowhere to go.

So this is a knowingly-wrong line, documented at the line, with the obvious fix
and its cost written next to it. The fix that works has to come from §2 — stop the
index writer publishing absence as deletion — after which a fresh joiner's index
write cannot subtract at all, and the suppression stops being load-bearing in
either direction.

## 0E. The PR gate's own sweep: main fails 20223, this tree fails chaos 6

Found 2026-10-04, by running CI's gate rather than reading its name. **Both trees
fail it, and which seed they fail on is the whole decision.**

The PR job (`ci.yml` -> `.github/actions/sync-stress`) runs the production fuzzer at
**128 seeds x 200 steps**, not the 400 x 300 this file's census uses, and
`run-sync-stress.sh --fuzz` — the command CLAUDE.md gives for local verification —
**does not run the production fuzzer at all.** That is why a locally green
`--fuzz` plus a green census still met a red gate. Run the composite action's
command too:

```sh
KNOTQ_FUZZ_SEEDS=128 KNOTQ_FUZZ_STEPS=200 cargo test -p knotq-app --release production_fuzz
```

| Tree | gate sweep | reproduces as a single seed? |
|---|---|---|
| `origin/main` | **20223** — a Daily page, its line and its binding lost | yes, deterministically |
| this tree | **chaos 6** — `device 4 still has 2 unpushed edit(s) after settling (wedged)` | only with the sweep's environment, below |

### Replaying a sweep seed needs the sweep's environment

chaos 6 passes as a plain `replay_production_seed` at 200 steps and fails in the
sweep, which looks like the process-dependence recorded below and is not. `run_seeds`
wraps the whole sweep in `with_fuzz_test_environment(true)`, so
`KNOTQ_SQUASH_MIN_STATE_BYTES=0` and `KNOTQ_SQUASH_MIN_RATIO=1` are set for every
seed and epoch squashes fire constantly. Add them and it reproduces every time, on
this tree and never on main:

```sh
KNOTQ_SQUASH_MIN_STATE_BYTES=0 KNOTQ_SQUASH_MIN_RATIO=1 \
  KNOTQ_REPRO_SEED=6 KNOTQ_FUZZ_STEPS=200 $BIN --ignored --exact \
  app::sync_service::production_fuzz::replay_production_seed
```

**Do this before calling any sweep result unattributable.**

### It is the move->index fix, and that fix is worth more than the seed

Ablated one change at a time against the gate sweep:

| Ablation | gate sweep |
|---|---|
| the integrity re-offer off | **worse** — 6 *and* 34 |
| the empty-index rescue off | 6 (unchanged) |
| **the move->index fix off** | **green, 33 passed, 0 failed** |

And with the move->index fix off, at census depth: chaos **194**, single **10054**
and single **10117** all fail — three reproducible content losses. So the gate can
be made green by giving back three data-loss bugs, which is the wrong trade.

### What is known about chaos 6, and what is not

Device 4's queue holds two `PersonalWorkspace` edits whose origin is
`Batch([])` — a synthetic operation, the shape `merge_sync_crdt_states` pushes when
it re-expresses a population. They do not drain.

A harness explanation was investigated and **falsified**: the settle loop injects
faults like any other sync, so it can crash a device in its last round and then
judge it for the pending edits that crash guarantees. Adding a bounded fault-free
drain before the verdict — tried in three positions, including after the passive
checks, since `view()` is what flushes deferred changes into countable pending edits
— leaves chaos 6 failing. Devices stay busy for all eight drain rounds. So this is
a real wedge, not an oracle artifact, and the harness change was reverted rather
than kept for the look of it.

What is still unexplained is why those two edits cannot be pushed. That is where the
next session should start, with the repro line above.

### Four attempts at it, and every one trades chaos 38 for chaos 109

Worth the space, because each looks like the obvious next idea and all four cost
the same seed. The target is to let a device that has not seen the account's index
PUBLISH what it holds while forbidding it to SUBTRACT — `sync_string_map` means
"the account's index is now exactly this", a claim only a writer that has seen the
account's index can make.

| Attempt | chaos 38 | chaos 109 |
|---|---|---|
| suppress the whole repair when the index was never pulled | fixed | **wedges** (also 142, 269 lose content) |
| add-only index write, witness `last_pulled_sequence > 0` | fixed | **wedges** |
| add-only, witness "a cursor exists at all" | still deletes | fixed |
| add-only, witness "pulled content OR accepted push" | fixed | **wedges** |
| …plus a containment comparison for add-only writers | fixed | **wedges** |

The third shows why a witness alone cannot do it: chaos 38's failed first run
leaves a cursor behind, so "has a cursor" is already true for the device that must
not subtract. The others all end at the same place, and the last one is the
interesting failure because it added the second half §2 asks for — a comparison
that reads a retained key as agreement rather than disagreement
(`workspace_folder_records_contain`) — and 109 wedged anyway.

**Traced:** device 4's runs report "pushed 1 doc(s), 0 pending left" and the oracle
still finds one unpushed index edit at settle. The retained entries **materialize
back into the plain workspace**, which is a local change, which queues another
index edit. That is the third change attempt A needed and never had: retention
needs a comparison *and* a materialization that does not adopt what was retained.
At that point this is attempt A rebuilt, with its 30-of-30 wedge waiting, so it was
reverted rather than finished.

**What that leaves as the choice**, and it is a real one rather than an oversight:
chaos 38 is an account-wide folder DELETION that is pre-existing and reachable in
the field; chaos 109 would be a NEW wedge — one device that stops syncing until it
signs out and in. Nothing measured here fixes the first without causing the second,
so the first is left standing and documented rather than traded for a regression.
The way out is §2 done properly — the index writer never publishing absence at all,
with the comparison and materialization that implies — which is a redesign of the
index write, not a patch to its callers.

Pinned by `a_fresh_install_whose_first_sync_failed_does_not_publish_its_own_index`
(production_fuzz, `#[ignore]`d because it fails).

**Not a `workspace_is_seeded()` check, which looks like the same idea and is not.**
A first sign-in runs `repopulate_workspace_canonically`, which seeds the canonical
index document with *this device's own* pre-sign-in content — so the document is
seeded while the device has still never seen the account's index. That is why
device 5 got past the `!workspace_is_seeded()` early return above.

### Why it was invisible until now

chaos 38 is green on `origin/main`: the trajectory there never puts a fresh
install's failed first run in front of a successful second one. The code path is
identical on main, so the defect is pre-existing and was simply never selected —
a reminder that the census measures *trajectories*, and a green seed is not a
proof about the code it ran. Seed 38 earned its place in the corpus the moment
0B's fix shifted the trajectory onto it.

Guarded against over-suppression by chaos **253**, the seed this file records as
depending on the repair actually running: it stays green.

## 0C. The document-namespaced item skeleton is written but NOT landed

Built before 2026-10-03, found uncommitted and unmeasured on that date, and held
back. It is the fix §"Also genuinely broken, and separate" asks for —
`stable_item_seed_client_id` hashing the item id alone, so the same item in two
scheme documents occupies one `(clientID, clock)` range — implemented as:

- `stable_item_seed_client_id(document, item_id)`, hash namespace `v1` → `v2`;
- `SCHEME_POPULATION_ENCODING_VERSION` 3 → 5, `ITEM_CREATION_ENCODING_VERSION`
  1 → 3, with both pinned hashes re-pinned;
- a `creation_candidate:<sha256>` key written into each item map, plus a
  `raw_content` field and a `reconcile_content_shadow` rule that uses the
  candidates to collapse a doubled creation run — the 10117 doubled-text gap.

**It is not the cause of the eight census regressions measured that day** (0B) —
reverting it leaves all eight failing — so it is held back on its own merits:

1. **Its own test was weakened rather than satisfied.**
   `item_skeleton_structs_must_not_alias_across_documents` asserted the property
   ("A's tombstone must not reach B's row"); the uncommitted version deletes that
   and asserts the implementation instead (the two clientIDs differ, the encoded
   states differ). Nothing in the tree demonstrates the aliasing is fixed.
2. **It is not backward compatible, and the version bumps do not make it so.**
   The constants exist so a build writing different bytes uses a different
   clientID and cannot alias an older build's structs — that is all they buy.
   The deterministic skeleton exists for the opposite reason: so two devices that
   independently create the SAME item encode byte-identical ops and Yjs dedupes
   them into one container. With derived (v8) item ids — starter content, daily
   carryover — independent same-id creation is routine, not rare.

   **Measured 2026-10-03 rather than argued, and it is worse than "a duplicate
   row":**

   | Two devices creating one row | rows | text |
   |---|---|---|
   | same build | 1 | `"shopping list"` |
   | old build + new build | 1 | **empty, in 20 of 40 sampled documents** |

   The row survives and its CONTENT does not, about half the time, and which way
   any one document goes is not predictable from anything a user can see. Each
   build's update carries its own skeleton *and* its text, so naively either
   container would arrive with its content — but the text is authored under
   `stable_item_creation_client_id`, which both builds derive identically, so the
   two text runs occupy the same `(clientID, clock)` range while hanging off
   different parents. Yjs keeps whichever integrated first; the two containers then
   compete for the single `items_by_id` key, resolved by last writer; and when the
   surviving container is not the one the surviving text attached to, the row goes
   blank. A starter line or a carried-over Daily line empties itself for everyone
   on one of the two versions.

   Pinned by `mixed_fleet_item_seed.rs`, which asserts the hazard (so it passes
   while the derivation is unchanged and fails the moment anyone changes it) and
   carries a control proving same-build dedupe still works.

So it needs what [[crdt-epoch-history-squash]] needed: every client updated
first, or a capability gate. Both halves are on `wip/uncommitted-2026-10-03`.
Measure it against the full 1200-seed census *and* an explicit old-encoding ↔
new-encoding merge test before landing any of it.

## 0y. The journal-loss gap closed itself; the witness was built, measured, and removed

`load_local_sync_state` marks `storage_recovery_pending` for the damage it can
see — an unparseable or empty file — and the recovery path re-expresses durable
tombstones before adopting the server's view. It does not cover a journal that is
simply **absent**: deleted, restored from a backup predating it, or defaulted by
an `unwrap_or_default()`. All three arrive looking exactly like a device that has
never synced, and the engine must treat those two opposite ways.

This entry used to propose the fix: a witness written once after the first
successful sync, outside the journal (`settings.json`), so "no cursors" could be
read as "the journal is gone" rather than "this device is new".

**Built it. Measured it. Removed it.** 2026-10-02:

- The whole thing was implemented and compiles end to end — an additive
  `AppSettings::sync_joined_at` keyed by `(api_base, user_id)`, a
  `#[serde(skip)]` `LocalSyncState::joined_account_at` the drivers inject from
  settings, recording after a run whose cursors landed, in the desktop sync task,
  the mobile sync cycle, the production fuzzer and the test harness.
- `an_offline_deletion_survives_an_unmarked_journal_loss`, the `#[ignore]`d test
  this entry existed to un-ignore, **passes without it**. It also passes with the
  witness forcibly set to `None`, and with the 0A rule disabled as well — so
  neither change is what fixed it.
- A second test was written specifically to reach the case the witness was
  designed for: the deleted row's id is **derived** (v8), which the first-join
  filter in `queue_local_only_documents_before_pull` deliberately refuses to
  re-assert, so by the argument above its deletion should be unrecoverable.
  `an_offline_deletion_of_a_derived_id_row_survives_an_unmarked_journal_loss`
  passes too, with and without the witness.

So in every case reachable from the harness, the deletion already survives —
the account's own copy of the document holds the tombstone and the integrity
proof (0x) brings the halves back together, with nothing having to infer that the
journal was lost.

**Why removing it was the right call and not laziness.** The witness's two
consumers are the two most dangerous flags in the engine:

- `first_sync_with_this_server` — flipping it false makes a device write its
  workspace index to the account instead of adopting the account's. Getting that
  wrong once cost the account everything (`offline_device_join.rs`).
- `needs_storage_recovery()` — flipping it true re-expresses durable tombstones
  and re-offers every document. Getting that wrong publishes a fresh install's
  starter tombstones onto the account's live rows (chaos seed 11) or drops an
  archived folder (production fuzz seed 20082). Both happened, from exactly this
  shape of inference.

A change to those flags with no failing case to justify it is unexercised risk in
the worst place in the codebase, and the tail risk is real: settings and the
journal are separate files, so a restore can leave a witness that says "joined"
beside cursors that say "new" — which is chaos seed 11's precondition.

**What a future attempt needs, in order.** First a *failing* case: a deletion
that is genuinely lost after an unmarked journal loss, which means a document the
server's integrity proof does not reach (the harness's server implements the
proof, so the harness cannot currently produce one). Then the witness, gated to
the `needs_storage_recovery` half only — re-offering is an idempotent Yjs union,
writing the index is not. The implementation is straightforward and is described
above; the missing piece is the evidence, not the code.

## 0z. `unlanded_pulls` is never cleared when a run lands

Found 2026-10-02 while reading a real `sync-state.json`: **164 entries**, from a
run that had long since landed.

`unlanded_pulls` is written in exactly one place (`sync_snapshot_in`) and read in
exactly one (`abandon_unlanded_sync_run`, on quit). Nothing clears it when the
run it describes lands in the UI store, so the file keeps naming that run's
documents until some later run overwrites the list. Quit while a *new* run is in
flight but before it has written its own list, and the shutdown rewinds the pull
cursors of up to 164 documents that were already landed — the next launch
re-downloads all of them.

Not data loss: `reset_pull_cursor` only sets `last_pulled_sequence = 0`, it does
not drop the cursor, so the device never masquerades as a first sync (which would
be far worse — see 0x). It is wasted bandwidth and a broken invariant: the field
claims to mean "the in-flight run's pulls" and actually means "the last run's
pulls, possibly landed long ago". The honest fix is to tag the list with the run
it belongs to and have the quit abandon it only when it matches the run actually
in flight.

## The two modeling choices behind the remaining bug families

Written 2026-10-02 after reading the code rather than the symptoms. Neither is a
bug in a function; both are decisions that make whole classes of bug reachable,
and the seeds this file keeps re-litigating are their shadows.

### 1. An item's placement is inferred from containment, and the tiebreak is per-device

`knotq_model::Item` has no field naming its scheme. An item's location *is*
"which document physically contains it", so a move is a tombstone in document A
plus an insert in document B — two operations in **two independent convergence
domains**, with per-document sequences on the server and per-document cursors on
the client. Nothing makes a replica observe both halves together, or ever.

The duplicate that results is resolved by `dedupe_materialized_items`:

```rust
let mut scheme_ids: Vec<SchemeId> = workspace.schemes.keys().copied().collect();
scheme_ids.sort();   // lowest id among the schemes THIS DEVICE has loaded
```

That is a function of the device's loaded window, not of account state. Replicas
pick different winners, each republishes its own belief, and
`reconcile_item_placements` then *deletes* the copy it judges to be losing.

**The code states the invariant it cannot keep, in the same breath.** Immediately
above that call:

> Keep the same deterministic winner on every replica. Only schemes materialized
> above participate: a lazy/off-window Daily page is intentionally absent and
> must not be interpreted as a deletion or placement decision.

Both sentences are correct and they contradict each other as an invariant: the
winner is deterministic *given a candidate set*, and the candidate set is
per-device. Two devices with different loaded windows — which is the normal
state, and the whole point of deferring off-window days — are choosing from
different sets. So "the same winner on every replica" is true only for replicas
that happen to have loaded the same days. Nothing in the design makes that so. This
is the 332 / 48 / 238 family, the "row is in no scheme at all" symptom, and the
reason chaos 127 above is so sharp-edged: a page restored from a deferred
document instantly creates a placement the live documents disagree with.

Representing a move as delete+insert is known not to converge — it is why
replicated-tree work defines a dedicated move operation, and why Figma, Linear,
Notion and Drive all store a parent as an **attribute of the child** resolved by
one authority. The restructure is to make placement an explicit, convergent
attribute so the winner is a pure function of account state, which also removes
the need for a destructive reconcile at all. It is an additive index/field
change: old clients ignore it and behave exactly as they do today.

#### Pinned, and why no device-local rule can fix it

`the_dedupe_winner_does_not_depend_on_which_schemes_a_device_loaded`
(`shared/sync/src/crdt/tests/workspace_materialization.rs`, `#[ignore]`d) is the
flaw in nine lines of setup: one row live in two scheme documents, two replicas,
one holding both schemes and one holding only the higher-id page. They disagree:

    the one holding both schemes shows it in  [46ff1310…]
    the one holding only the higher shows it in [a02e6bfa…]

That also closes off the cheap fixes. A rule can only converge if it reads data
both replicas have, and what differs here *is* the data they have — one of them
cannot see the low-id page at all. So no amount of re-deriving the winner from
local documents converges, however the tiebreak is phrased. **The placement has
to be stored somewhere both replicas read.** Use this test as the acceptance
criterion; it passes exactly when that is true.

#### The placement attribute, attempted

Built and measured 2026-10-03, then reverted. Worth reading before anyone builds
it again, because it very nearly worked and the reason it did not is specific.

What was built: an additive `item_home` root map in the workspace index
(`item_id -> scheme_id`), written by the device that performs a cross-scheme move
(`crdt_change_set_for_command` detects delete-from-A + insert-into-B), read by
`dedupe_materialized_items`, which preferred the recorded home over its
lowest-loaded-id rule. No on-disk format change: the plain workspace never saw
it, and an older build ignores an unknown Yjs root map exactly as it ignores
`node_fields`.

It worked on the seeds: chaos 194, 389, 48, 238 and single-account 10054 all went
green. Three implementation details were load-bearing and are worth keeping:

- The claim must be written **inside** `sync_snapshot`'s delta-capture window.
  The baseline is taken there, so a claim written beforehand is excluded from the
  emitted update and never reaches the server — and a run whose only change is a
  claim has to emit something.
- The claim must be honoured **only as a preference among copies that exist**. A
  first version hid the row whenever its claimed scheme was unloaded or had since
  tombstoned it, so the line vanished; chaos 127 reported it immediately as the
  server losing `…0402`.
- `seen.insert` still decides even in the home scheme. Skipping it left both
  copies visible when one scheme held an id twice, which is a workspace holding
  one id twice — seven single-account seeds caught that at once.

**Why it was reverted:** even with all three, seeds 10044 and 135 ended with the
VISIBLE workspace holding a row twice. The plain workspace is reconciled against
the dedupe by a separate pass (`reconcile_item_placements`, gated on the
landing), and a claim can disagree with what that pass last wrote, so the two
halves drift until it runs again. Closing that means making the reconcile
claim-aware and re-examining its "the winner is a minimum, so the globally lowest
copy is never deleted anywhere" safety argument, which a claim invalidates.

Then the ablation that ended it: with the claims forced empty the same seeds
still passed, because the attempt had *also* been setting `workspace: true` on
every move — and that alone is the fix above. The attribute was solving a problem
the index write already solved, at far greater cost.

The flaw it targets is still real and still pinned
(`the_dedupe_winner_does_not_depend_on_which_schemes_a_device_loaded`): two
replicas with different loaded windows still disagree about where a duplicated
row lives. What is no longer true is that any *census seed* depends on it.

#### The restructure, scoped so it needs no on-disk format change

Worked out 2026-10-02. The obvious shape — put a `scheme` field on `Item` — drags
in `workspace.json`, which means the upgrade framework, a release fixture and a
migration. None of that is necessary, because the plain workspace is a
*projection*: the authority can live in the CRDT alone.

1. **A new root map in the workspace index document**, `item_home`, mapping
   `item_id -> scheme_id`. Yjs merges an unknown root map without complaint, so
   an older build ignores it and keeps using today's rule — no worse than now.
2. **Write an entry only when a row crosses schemes.** A row that has never moved
   is unambiguous and needs no entry, which keeps the map proportional to moves
   rather than to items (a 3.7k-item workspace would otherwise add 3.7k index
   keys that every device pulls).
3. **Retain those keys additively.** `sync_string_map` would delete every entry a
   writer does not list, which is the flaw in §2 — but `item_home` is safe to
   retain where `nodes` was not, because nothing compares it against the plain
   workspace. That comparison is what made retention wedge 30/30 seeds; a map
   outside it has no fixed point to lose.
4. **`dedupe_materialized_items` prefers `item_home` when present**, falling back
   to the current lowest-loaded-id rule when it is absent. That is the whole
   behavioural change: the winner stops depending on which schemes the device has
   loaded, so replicas agree, nobody republishes a competing placement, and
   `reconcile_item_placements` no longer has to delete anything.
5. **Nothing is written to `workspace.json`.** `materialize_workspace_inner`
   already holds the index document when it calls the dedupe, so the map is read
   there and passed down; it never needs a home on `Workspace`.

Acceptance: seeds 194, 10054 and 20223 (the whole census failure set), plus 48
and 238 should stop needing the placement-reconcile gate to stay narrow. Gate:
the full per-seed census, because the sweep is not trustworthy, and a check that
chaos 112/127/277 stay green — those are the three seeds that caught the rescue
attempts, and they are the ones a placement change is most likely to disturb.

Not attempted here. It is a change to the index writer and the materializer — the
two places where today's three attempts each produced a regression that only a
1200-seed census caught — and it deserves its own run at it rather than the tail
of a session.

### §2 ATTEMPTED PROPERLY, 2026-10-04: both halves built and measured

The first time the two changes this section says are needed have been built
*together* and measured at both depths. Not landable, and the numbers say exactly
why — read these before attempting it a third time.

**Half one, retention.** `sync_string_map` grew a sibling,
`sync_string_map_removing`, where the writer must justify each removal with
`may_remove(key, stored)`. The evidence is the permanent-delete tombstone the
section already identifies — `permanently_deleted_scheme_ids` /
`permanently_deleted_folder_ids`, both already computed in `replace_snapshot`.
Applied to `nodes`, `node_fields`, `scheme_sync`, `folder_sync` and `daily_queue`
(the last keyed on the *bound scheme's* tombstone, since a day this device has not
loaded must keep its binding).

Alone it is catastrophic, and it reproduces "Attempt A" at full scale:

| Configuration | failing seeds, gate sweep 128x200 |
|---|---|
| single-account | **126 of 128** |
| journal-loss | **126 of 128** |
| chaos | **65 of 128** |

Every one is the wedge, on every device at once: `device 0 still has 10 unpushed
edit(s) after settling`, `device 1 … 16`, `device 2 … 8`, `device 3 … 5`, all on the
`PersonalWorkspace` document. The lost fixed point, exactly as recorded.

**Half two, the comparison.** `workspace_folder_records_contain` asks whether the
document holds everything the plain workspace does, instead of whether they are
equal, and `workspace_index_mismatch` uses it. The document is *expected* to hold
more once writes retain.

**And §1 was then built on top of it, so the ordering claim below is measured too.**
Everything on `spike/placement-claim-and-index-retention`: the `item_home` map, the
claim-aware dedupe, and — the part the earlier §1 attempt skipped — a claim-aware
`reconcile_item_placements`, which may only tombstone a claimed row on a replica that
can SEE the claimed home. That restates the destructive half's safety argument
instead of inheriting one a claim invalidates, and it is why
`the_dedupe_winner_does_not_depend_on_which_schemes_a_device_loaded` is un-ignored
and passing there, for the first time, with a control test proving the claim is doing
the work.

The full matrix, every combination at both depths:

| Tree | gate sweep 128x200 | census 400x300 |
|---|---|---|
| `origin/main` | 1 — journal 20223 | 5 |
| the branch that landed | 1 — chaos 6 | **1** — chaos 38 |
| §1 only | 3 — single 10118, journal 20116, chaos 6 | not run |
| §2 only | 2 — single 10000, journal 20037 | 14 |
| §1 + §2 | 2 — chaos 30, journal 20100 | 13 |

§1 + §2 is the only thing that removes **chaos 6 and chaos 38 together**, and it takes
single-account to 0/128 at gate depth. It is still 13x worse than the landed branch at
census depth, and chaos **253** and **295** recur across configurations, so a fourth
part is missing beyond claim + retention + comparison. Do not read the ordering
conclusion above as "§1 unlocks §2": §1 makes §2 better (14 -> 13 at census, and the
seeds it fixes are the ones that matter most) and neither order is sufficient alone.

Together they are a different world — **317 failing seeds become 2**:

| Tree | gate sweep (128x200) | census (400x300) |
|---|---|---|
| retention only | 317 | not run (pointless) |
| retention + containment | **2** — single 10000, journal 20037 | **14** |
| `origin/main` | 1 — journal 20223 | 5 |
| the branch that landed | 1 — chaos 6 | 1 — chaos 38 |

Chaos goes **fully green at the gate's depth, seeds 6 and 38 included** — the first
thing in this file to fix either. But at census depth it costs chaos 32, 118, 213,
**253**, 269, 285, 295, **389**, single 10000, 10071, 10128 and journal 20037, 20062,
20357. 253 is the seed this file records as needing the repair to RUN and 389 is one
the move->index fix had fixed, so retention disturbs both directions.

**What the remaining 14 have in common, and it names half three.** Every one
replays as the same shape: `device N's sync (server state) lost item X (in scheme Y
"Daily …")` — the audit device, materializing from the server, cannot see a row in a
Daily page. Retaining an index entry makes a page visible to a materializer whose
documents do not hold it; the row then appears in two schemes,
`dedupe_materialized_items` picks one, and the oracle's one-entry-per-id view reports
it lost from the other. That is §1, reached from §2: **retention cannot land until
placement is an explicit convergent attribute, because retention manufactures
exactly the duplicate placements §1 cannot resolve.**

So the order is settled by measurement: **§1 first, then §2.** The reverse — which is
the intuitive order, since §2 is where the data loss is visible — produces a tree
that is better at 200 steps and five times worse at 300.

### 2. The workspace index publishes absence as deletion

```rust
let stale = map.keys().filter(|key| !desired_keys.contains(*key));
for key in stale { map.remove(&mut *txn, &key); }   // sync_string_map
```

Every index write says "the account's index is now exactly this". A device with a
partial view — an unloaded Daily page, a scheme the pull just dropped — deletes
the rest for everyone, which is the set-reset anti-pattern and the 194 / 10054 /
20223 family. It is also self-reinforcing: the pull drops a scheme the account's
index lacks, `ensure_sync_metadata` drops its `scheme_sync` entry, and the next
index write makes the loss authoritative.

The intended design is already written down — `PermanentlyDeleteScheme`'s own
comment says the tombstone exists so "the workspace-index writer [can] tell a
real deletion apart from a scheme this device merely cannot see" — the writer
just never used it for the removal decision. Attempt A above is what happens if
you add that check alone; see it for what else has to change with it.

### Also genuinely broken, and separate

`stable_item_seed_client_id` hashes the item id alone while every other derived
identity namespaces by `DocumentId`, so the same item in two documents occupies
an identical `(clientID, clock)` range — two distinct operations sharing one Yjs
identity, which is the invariant Yjs correctness rests on. Needs an
encoding-version bump and a mixed-fleet decision, so it is called out rather
than slipped in.

## The per-seed census, and the three seeds that survive it

**The sweep is not the instrument.** It misses genuinely failing seeds and
reports ones that do not reproduce (next section). One seed per process is
reproducible — seed 10350 passed 40/40 and seed 10374 0/100 that way — so the
measurement that counts is a census: every seed in every configuration, one
process each, at the nightly's own depth.

**Check the binary before trusting a census.** `ls -t … | head -1` picks the
newest matching file, and `target/release/deps/` accumulates one `knotq-<hash>`
per distinct source state *plus* the `knotq` bin target under the same prefix. Run
`"$BIN" --list | grep -c production_fuzz` first and assert it is non-zero: a
census whose binary is stale or is the app rather than the harness reports every
seed green. On 2026-10-03 three consecutive censuses reported single-account
0/400 while seed **10395** fails 10/10 when replayed — on this tree *and* on
`origin/main`. Same lesson as the two nightly steps above, from a third
direction: read what the instrument actually ran.

```sh
# ~45 min for 1200 seeds; the script is in the session notes, the shape is:
BIN=$(ls -t target/release/deps/knotq-* | grep -vE '\.(d|rlib|rmeta)$' | head -1)
KNOTQ_REPRO_SEED=$seed KNOTQ_FUZZ_STEPS=300 $BIN --ignored --exact \
  app::sync_service::production_fuzz::replay_production_seed      # +KNOTQ_REPRO_PLAIN=1
KNOTQ_REPRO_SEED=$seed KNOTQ_FUZZ_STEPS=300 $BIN --ignored --exact \
  app::sync_service::production_fuzz::replay_journal_loss_seed
```

**Census on 2026-10-02, 400 seeds per configuration at 300 steps:**

| Configuration | Failing | After the 2026-10-03 fix |
|---|---|---|
| chaos (1–400) | **194** | none |
| single-account (10000–10399) | **10054** | none |
| journal-loss (20000–20399) | **20223** | **20223** |

Three seeds, one per configuration — far smaller than the sweep's shifting
reports suggested. Two of them were one bug; see "[FIXED] The whole family was a
cross-scheme move not touching the index".

### 20223's root cause: a device can never publish its own index

Measured, not inferred. Device 2 relaunches at step 31 and its next sync drops
Daily 2026-09-16 with its line and its queue binding
(`published=false unpushed=true archived=false daily=true`). The pre-pull repair
— the thing that publishes local index content — is skipped, and instrumenting
the guard says why:

    skipped: workspace document not seeded yet (doc=051f6cea… state_bytes=2 cursors=6 schemes=6)

**A 2-byte (empty) index document on a device holding 6 cursors and 6 schemes.**
That is a deadlock, not a transient:

1. `queue_local_only_documents_before_pull` returns early while
   `!crdt_docs.workspace_is_seeded()` (the index has no `meta.id`), for the good
   reason that writing the plain workspace into an unseeded index mints a
   competing index that wins the merge and costs the account everything
   (`offline_device_join.rs`).
2. An index document becomes seeded by being **written** locally, or by pulling
   an index that already has content.
3. So a device whose own index is empty while the account's is also empty can
   never publish its index — and the moment the account gets index content from
   another device, this device's local-only pages are materialized away.

The `!daily` arm of `restore_unpublished_schemes_dropped_by_pull` is what
finally drops the page, but it is the last step of that chain, not the cause.
**The fix belongs at step 1–2**: either seed the index document on the first
successful sync (`queue_workspace_bootstrap_updates` already exists for this —
why it did not seed device 2 is the open question), or publish local index
content *after* adopting the server's index, which is sequencing rather than
suppression. The post-pull repair re-expresses scheme content only
(`sync_scheme_documents`), so publishing the index there is new machinery.

Note also that most "not seeded yet" lines are benign: 82 of 83 in that run come
from the fuzzer's own audit-pull device, which starts empty every time
(`cursors=0 schemes=0`). Only the one reading above has real content. Do not
read the raw count as a systemic hole; it was checked and it is not one.

### [FIXED] The whole family was a cross-scheme move not touching the index

**Resolved 2026-10-03. Census: chaos 0/400, single-account 0/400, journal 1/400
(only 20223 left).** The fix is three lines in
`crdt_change_set_for_command`, and it is none of the things attempted below.

`Command::crdt_documents()` reports `workspace: false` for a move batch —
correct about the document *set*, since a move adds no scheme and removes none,
and the wrong question. The index is how every other device learns the
destination page exists at all: its node entry, and for a Daily page its queue
binding. A move that leaves the index out of the change set leaves the
destination unpublished, so the next pull materializes the account's index over
it and the page, its lines and its binding go together.

```rust
let moves_a_line_between_schemes = /* an id deleted from one scheme, inserted in another */;
WorkspaceCrdtChangeSet {
    workspace: documents.workspace || moves_a_line_between_schemes,
    ...
```

Fixed by this, all previously red: chaos **194**, **389**, **48**, **238** and
single-account **10054**. Nothing regressed — 112, 127, 135, 277 and 10044 stay
green, and the whole 1200-seed census is clean apart from 20223.

**It costs no extra traffic.** `workspace: true` only emits an update when the
index's content actually differs; for a move with nothing unpublished the write
is a no-op. That is also why it is safe: it gives pending local index content a
chance to flush, and does nothing otherwise.

**Pinned by** `desktop/state/tests/move_publishes_the_index.rs` — one test that
the move puts the index in scope (fails without the change) and one guard that an
ordinary text edit does not (the index is pushed to every device, so widening
that would turn a keystroke into account-wide traffic).

**It also unblocked something this file said was blocked.** 0w records that
widening the placement-reconcile gate "takes the release-depth gate from 5
failing seeds to 17" and surfaces chaos 48/238. Measured again today: with 0A in
place it surfaces neither, because 0A removed the resurrection that drove the
oscillation. The gate widening is *not* in the tree — it turned out to be
unnecessary once the move published the index — but the note about it was
measured before 0A and should not be trusted as written.

### Three attempts that did NOT fix it, and what each cost

Kept because each one is a plausible-looking fix that a reader will propose
again, and each was caught only by running the full census rather than the seed
it targeted.

| Attempt | Fixed | Broke |
|---|---|---|
| index writes retain keys with no delete tombstone | 194 | **30 of 30** sampled green seeds wedge |
| daily rescue restores the page body | 20223 | 127 — a row shows twice |
| daily rescue restores the binding only | 20223 | 112, 277 — **duplicate item ids** |
| the placement attribute (`item_home` in the index) | 194, 389, 48, 238, 10054 | 10044, 135 — **duplicate item ids in the visible workspace** |

The fourth is the interesting one, and it is written up under "the placement
attribute, attempted" below. The first three share a cause: each puts back a
page, binding or index entry the account has not seen, and since placement is an
inference, reintroducing a container reintroduces a placement claim the documents
have already moved past.

### The rescue layer, for the record

**Three independent attempts, each measured against the full 1200-seed census,
each regressing a different seed for the same underlying reason.** This is the
strongest statement in this file about 194 / 10054 / 20223, and it is a negative
result worth more than another patch: it closes off a whole approach.

| Attempt | Fixes | Breaks | Symptom of the regression |
|---|---|---|---|
| index writes retain keys with no delete tombstone | 194 | **30 of 30** sampled green seeds | every device wedges with an undrained index queue |
| daily rescue restores the page body | 20223 | 127 | a row shows twice; visible 15 vs the device's own CRDT 16 |
| daily rescue restores the binding only | 20223 | 112, 277 | **duplicate item ids in the visible workspace** |

The third is the clearest. Re-binding a day whose page the account has never held
makes the page reachable again — and its rows now live somewhere else, so the
same id becomes visible twice:

    step 175: device 0: sync left item c1eedd3f… in the workspace more than once
    step 187: scheme 2b1646c2… has 3 item(s), the CRDT has 2; only in the
              workspace: [c1eedd3f… (the CRDT places it in 1d98a5db…)]

An `items_by_id` map has one entry per id, so a workspace holding an id twice has
no CRDT representation at all — the halves diverge from that moment on. That is a
worse failure than the one being fixed.

**Why all three fail the same way.** Every one of them tries to put back a page,
a binding or an index entry that the account has not seen, and an item's
placement is not stored anywhere — it *is* which document contains the row (see
"The two modeling choices"). So reintroducing any container reintroduces a
placement claim, and the documents have moved on. There is no version of
"restore what the account is missing" that is safe while placement is an
inference.

**What this means for 194 / 10054 / 20223.** They are one bug with one fix, and
the fix is the placement restructure, not the rescue. Until an item carries its
owning scheme as an explicit convergent attribute:

- a rescue cannot know whether the rows it is restoring still belong to the page;
- `dedupe_materialized_items` cannot agree between devices, because its winner is
  the lowest scheme id *among the schemes that device happens to have loaded*;
- and the repair that would publish a device's local-only page is the same code
  that, run a moment later, has to decide whether that page's rows are duplicates.

Do not spend another attempt at this layer. The next change here should be the
attribute, with these three seeds as its acceptance test and the full census as
its regression gate.

### Two earlier fixes attempted against this census, and both reverted

Both were measured against the full census rather than the seeds they targeted,
which is the only reason they are recorded as failures instead of shipped.

**A. Index writes stop publishing absence as deletion.** `sync_string_map` means
"the account's index is now exactly this", so a device with a partial view
deletes the rest for everyone. Making it retain any key the writer has no
permanent-delete tombstone for **fixes chaos 194** and **wedges 30 of 30**
otherwise-green single-account seeds: every device ends with an undrained index
queue ("still has 12 unpushed edit(s) after settling (wedged)"). The mechanism is
a lost fixed point — a retained entry materializes back into the workspace, the
plain and CRDT halves can then never re-converge, so `workspace_index_mismatch`
is true on every pull and the repair queues another full index snapshot forever.

A correct version needs **two** changes together: retention, *and* a comparison
that does not read a retained-but-unspeakable-for key as "the halves disagree".
The permanent-delete sentinel alone is also too strict a removability test —
archive flows remove a node with an ordinary origin. Pinned as an `#[ignore]`d
test in `shared/sync/tests/index_absence_is_not_deletion.rs`, whose sibling
(`an_explicit_delete_still_removes_the_scheme_everywhere`) is the guard any
attempt has to keep passing.

**B. Narrow the daily arm of the dropped-scheme rescue.** The rescue declines for
*any* Daily page; `held_schemes` is built from the pre-pull plain workspace, so an
out-of-window day can never be a candidate, which suggested the guard was wider
than its reason. Restricting it to days the device has no unpublished stake in
(`items` is captured only when the document has unpushed edits the server has
never held) **fixes 20223** and **breaks chaos 127** — exactly the seed the
guard's own comment names. The full census makes it a 1:1 trade:
chaos {127, 194} + single {10054} + journal {} against the baseline's
{194, 10054, 20223}.

127's mechanism, measured with `KNOTQ_DBG_DUP=1`: row `…2005` is live in
**exactly one** CRDT document, so this is not a dedupe hidden copy. The restore
reintroduces the row onto the page it has left, from a body read out of a
**deferred** document — which `documents_holding_item` does not scan — so the
device ends up showing the row twice, the dedupe hides the live copy, and from
step 188 the visible page has 15 rows against its own CRDT's 16. Filtering rows
already placed elsewhere does not help: the restore happens *before* the move.

**The guard is load-bearing and its comment is correct.** A rescue must not
rebuild a page from a deferred document's body.

## The sweep's answer depends on the PROCESS, not on the seed

Sharpened 2026-10-02, and it changes how every result in this file should be
read. The section below has said since 0v that "something those share changes a
seed's outcome"; the sharing is now bounded, and it is **not** inside a sweep.

Measured on single-account seed 10350, which the nightly-depth sweep reports as
failing:

| How it was run | Result |
|---|---|
| the nightly's `cargo test production_fuzz` (all three sweeps, one process) | **fails** at step 202 |
| the single-account sweep alone, 400 x 300, `KNOTQ_FUZZ_WORKERS=4` | passes |
| the single-account sweep alone, 400 x 300, `KNOTQ_FUZZ_WORKERS=1` | passes |
| one-seed replay, 40 separate processes | passes 40/40 |

And then, running the identical tree at the identical depth a second time, the
same sweep reported **10374 instead of 10350**. Replaying both one at a time:

| Seed | Sweep run 1 | Sweep run 2 | One-seed replay |
|---|---|---|---|
| 10054 | fails | fails | **fails** |
| 10350 | fails | passes | **passes** (40/40 processes) |
| 10374 | passes | fails | **fails** |

So the sweep **misses a genuine failing seed** (10374 in run 1) and **reports one
that does not reproduce** (10350 in run 1), in the same run. 10374 is real: four
violations at step 248 — device 3 loses two schemes and a folder no device
deleted — and it fails identically with the 0A rule disabled, so it is not a
regression from it. It had simply never been named, because the sweep that was
supposed to name it did not.

Identical results at one worker and at four rule out concurrency *within* a
sweep. Passing 40/40 as its own process rules out per-process `HashMap` ordering
in the replay configuration. What is left is residue from the **other sweep tests
in the same process** — `ENV_LOCK` serializes them, so they do not overlap in
time, yet running after them changes seed 10350's outcome.

Also eliminated, each by measurement rather than argument:

- **Not the squash thresholds.** `run_seeds` sets `KNOTQ_SQUASH_MIN_STATE_BYTES=0`
  and `KNOTQ_SQUASH_MIN_RATIO=1` process-wide for a whole sweep even for seeds
  whose own `maintenance_coverage` is off, and `replay_production_seed` does not.
  Replaying 10350 with those set by hand still passes.
- **Not ids minted off the seeded thread.** `set_deterministic_id_seed` is
  thread-local and `parallel::map_ordered` spawns its own threads, so an id minted
  inside one would fall back to a real `Uuid::new_v4()` and make the run
  nondeterministic. Instrumenting that fallback arm to print a backtrace whenever
  it is taken shows **zero** hits across a 300-step replay. The production code
  does not mint ids on those threads.
- **Not the deterministic-id memo** in `knotq_model::daily_queue`: it is a
  thread-local cache of a pure function of the date.

**What this means for the gate.** A sweep failure is not by itself attributable
to its seed, and a green replay does not exonerate the code. Both directions have
now been observed. Until the residue is found, report sweep results **and** a
one-seed replay, and say which disagreed — several conclusions in 0w rest on
"the seed replays green", which is exactly the evidence this undermines.

**Where to look next.** Something process-global that the production code (not the
harness) carries across `World` instances: a `OnceLock`/`static` initialised on
first use, a cached path or policy read once per process (`write_atomic`'s
fsync policy is one such, already deliberate), or a counter like `fuzz_root`'s
`RUN`, which differs between the gate run and an isolated sweep because the other
sweeps advanced it. The cheapest next experiment is to run two sweeps in one
process and bisect which predecessor is required.

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
