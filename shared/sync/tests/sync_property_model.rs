//! Randomized, model-based sync fuzzer — the closest thing to a *guarantee* that
//! sync converges no matter the scenario.
//!
//! It runs many deterministic seeds. Each seed builds a world of several accounts
//! (each its own server) and several devices, then applies a long random sequence of
//! operations drawn from EVERY feature — schemes, lines, folders, nesting, moves,
//! archive/restore, delete, daily queue, media, item indent — interleaved with
//! **signing a device out of one account and into another** and syncing. After the
//! random phase it settles every account and asserts the invariants that a correct
//! sync must always uphold:
//!
//!   1. Every device ends fully pushed (including after any self-healed rejection).
//!   2. All devices currently on the same account converge to identical content.
//!   3. A brand-new device signing into each account sees exactly that content
//!      (the server holds the full, materializable state — no silent loss).
//!
//! Seeds are deterministic, so any failure prints a seed that reproduces the exact
//! operation sequence. Crank coverage with `KNOTQ_FUZZ_SEEDS` / `KNOTQ_FUZZ_STEPS`.

mod common;

use chrono::{DateTime, NaiveDate, Utc};
use common::{Rng, TestDevice, TestServer};
use knotq_model::{
    DocumentId, FolderId, Item, ItemMarker, ReplicaId, SchemeId, SyncDocumentKind, Workspace,
    WorkspaceId,
};
use knotq_sync::{
    batch_pull_and_apply, BatchPullRequest, BatchPullResponse, BatchPushRequest, BatchPushResponse,
    PulledCrdtDocument, SyncTransport, WorkspaceCrdtDocuments,
};
use std::cell::Cell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};

fn fresh_device(account: WorkspaceId) -> TestDevice {
    let mut base = Workspace::new();
    base.canonicalize_personal_sync_identity(account);
    TestDevice::new_from_base(&base, account)
}

fn date_for(n: u64) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 7, 1 + (n % 28) as u32).unwrap()
}

/// Human-readable content lines for diffing two diverged devices: each scheme as
/// `scheme <id> "<name>" archived=<bool> items=[texts]` and each folder.
fn fingerprint(dev: &TestDevice) -> Vec<String> {
    let mut out = Vec::new();
    for (id, scheme) in &dev.workspace.schemes {
        let texts: Vec<String> = scheme.items.iter().map(|it| it.text()).collect();
        out.push(format!(
            "scheme {id} {:?} archived={} items={texts:?}",
            scheme.name,
            dev.workspace.recently_deleted.contains(id)
        ));
    }
    for folder in dev.workspace.folders.values() {
        out.push(format!(
            "folder {} {:?} children={}",
            folder.id,
            folder.name,
            folder.children.len()
        ));
    }
    out.sort();
    out
}

struct Account {
    workspace: WorkspaceId,
    server: TestServer,
    url: String,
}

struct DeviceSlot {
    dev: TestDevice,
    account: usize,
    /// Per-device undo/redo stacks of prior scheme-content snapshots, used only
    /// when `World::enable_undo` is set. Each entry is a scheme and the items it
    /// held before an edit; undo reverts to it, redo reapplies.
    undo: Vec<(SchemeId, Vec<Item>)>,
    redo: Vec<(SchemeId, Vec<Item>)>,
}

struct World {
    accounts: Vec<Account>,
    devices: Vec<DeviceSlot>,
    rng: Rng,
    step_no: usize,
    trace: bool,
    /// When set, `step` may also undo/redo. Gated so the extra RNG draw happens
    /// only in the undo fuzz test — the other seeds keep their exact sequences.
    enable_undo: bool,
    /// When set, `step` may also restart a device. Gated for the same reason as
    /// `enable_undo`: enabling it unconditionally would shift every existing
    /// seed's operation sequence, including the named regression seeds.
    enable_restart: bool,
    /// When set, `step` may run the server's at-rest compaction sweep between
    /// syncs (transcode every stored `state_v1` v1->v2->v1 without bumping seq).
    /// Gated like the others so existing seeds keep their RNG sequence. The
    /// wedge this guards against: a compaction that is not state-vector-
    /// preserving makes every device that pulled before the sweep fail the pull
    /// integrity check forever.
    enable_compaction: bool,
}

/// Ops in `edit_op` that mutate a scheme's item list (so an undo can revert it).
fn is_content_op(op: u64) -> bool {
    matches!(op, 5 | 6 | 7 | 8 | 9 | 11 | 20 | 24 | 25)
}

impl World {
    fn new(seed: u64, num_accounts: usize, num_devices: usize) -> Self {
        // Deterministic ids + sorted selection below make a seed fully reproducible
        // (random v4 ids otherwise make a failing run impossible to replay/debug).
        // KNOTQ_DIAG_RANDOM restores random ids for the diagnostic collector, which
        // needs the chaotic interleavings to surface the rare divergence.
        if std::env::var("KNOTQ_DIAG_RANDOM").is_ok() {
            knotq_model::set_deterministic_id_seed(None);
        } else {
            knotq_model::set_deterministic_id_seed(Some(seed));
        }
        let accounts: Vec<Account> = (0..num_accounts)
            .map(|i| Account {
                workspace: WorkspaceId::new(),
                server: TestServer::default(),
                url: format!("memory://account-{i}"),
            })
            .collect();
        let devices: Vec<DeviceSlot> = (0..num_devices)
            .map(|i| {
                let account = i % num_accounts;
                DeviceSlot {
                    dev: fresh_device(accounts[account].workspace),
                    account,
                    undo: Vec::new(),
                    redo: Vec::new(),
                }
            })
            .collect();
        Self {
            accounts,
            devices,
            rng: Rng::new(seed),
            step_no: 0,
            trace: std::env::var("KNOTQ_FUZZ_TRACE").is_ok(),
            enable_undo: false,
            enable_restart: false,
            enable_compaction: false,
        }
    }

    fn log(&self, msg: &str) {
        if self.trace {
            eprintln!("[{:>4}] {msg}", self.step_no);
        }
    }

    /// One full sync cycle for a device against its current account: CRDT pull/push
    /// plus media upload/download (media is a separate transfer from the CRDT batch).
    fn sync_device(&mut self, i: usize) -> anyhow::Result<()> {
        let account = self.devices[i].account;
        let trace = self.trace;
        let server = &self.accounts[account].server;
        let dev = &mut self.devices[i].dev;
        dev.try_sync(server)?;
        let remote_latest = dev.remote_latest_after_sync();
        dev.upload_media_to(server, &remote_latest)?;
        dev.download_media_from(server);
        if trace {
            for skipped in &dev.last_skipped {
                eprintln!(
                    "SKIP dev{i} acct{account} doc={} unknown_scheme={} reason={}",
                    skipped.document, skipped.unknown_scheme_document, skipped.reason
                );
            }
        }
        Ok(())
    }

    /// Sign device `i` out of its account and into a different one.
    fn switch_account(&mut self, i: usize) {
        if self.accounts.len() < 2 {
            return;
        }
        let current = self.devices[i].account;
        let mut target = self.rng.below(self.accounts.len() as u64) as usize;
        if target == current {
            target = (target + 1) % self.accounts.len();
        }
        let workspace = self.accounts[target].workspace;
        let url = self.accounts[target].url.clone();
        self.devices[i].dev.switch_account(workspace, &url);
        self.devices[i].account = target;
    }

    fn edit_op(&mut self, i: usize) {
        // Sort the keys: HashMap iteration order is per-process random, so without this
        // the choice (and thus the whole run) would not be reproducible from the seed.
        let mut scheme_ids: Vec<SchemeId> = self.devices[i]
            .dev
            .workspace
            .schemes
            .keys()
            .copied()
            .collect();
        scheme_ids.sort();
        let mut folder_ids: Vec<FolderId> = self.devices[i]
            .dev
            .workspace
            .folders
            .keys()
            .copied()
            .collect();
        folder_ids.sort();
        let root = self.devices[i].dev.workspace.root;

        // Pre-roll every random choice BEFORE taking &mut on the device, so the RNG
        // and the device (disjoint fields) are never borrowed at once.
        let op = self.rng.below(27);
        let scheme = (!scheme_ids.is_empty())
            .then(|| scheme_ids[self.rng.below(scheme_ids.len() as u64) as usize]);
        let folder = (!folder_ids.is_empty())
            .then(|| folder_ids[self.rng.below(folder_ids.len() as u64) as usize]);
        let nonroot_folder = {
            let nr: Vec<FolderId> = folder_ids.into_iter().filter(|f| *f != root).collect();
            (!nr.is_empty()).then(|| nr[self.rng.below(nr.len() as u64) as usize])
        };
        let a = self.rng.below(100_000);
        let b = self.rng.below(100_000);

        self.log(&format!(
            "dev{i} acct{} EDIT op{op} scheme={:?}",
            self.devices[i].account,
            scheme.map(|s| s.to_string())
        ));
        // Record the pre-edit content so the undo model (when enabled) can later
        // revert to it. This draws no RNG, so seeds without undo are unaffected.
        if self.enable_undo && is_content_op(op) {
            if let Some(s) = scheme {
                if let Some(items) = self.devices[i].dev.scheme_items_snapshot(s) {
                    self.devices[i].undo.push((s, items));
                    self.devices[i].redo.clear();
                }
            }
        }
        let dev = &mut self.devices[i].dev;
        let items_in = |s: SchemeId| dev.workspace.schemes.get(&s).map_or(0, |x| x.items.len());

        match op {
            0 | 1 => {
                dev.add_scheme(&format!("scheme-{a}"), &["seed"]);
            }
            2 => {
                dev.add_folder(&format!("folder-{a}"));
            }
            3 => {
                if let Some(p) = folder {
                    dev.add_subfolder(p, &format!("sub-{a}"));
                }
            }
            4 => {
                if let Some(p) = folder {
                    dev.add_scheme_to_folder(p, &format!("fscheme-{a}"), &["seed"]);
                }
            }
            5 | 6 => {
                if let Some(s) = scheme {
                    dev.append_line(s, &format!("line-{a}"));
                }
            }
            7 => {
                if let Some(s) = scheme {
                    let n = items_in(s);
                    if n > 0 {
                        dev.edit_line(s, (a as usize) % n, &format!("edit-{b}"));
                    }
                }
            }
            8 => {
                if let Some(s) = scheme {
                    let n = items_in(s);
                    dev.insert_line(s, (a as usize) % (n + 1), &format!("ins-{a}"));
                }
            }
            9 => {
                if let Some(s) = scheme {
                    let n = items_in(s);
                    if n > 0 {
                        dev.remove_line(s, (a as usize) % n);
                    }
                }
            }
            10 => {
                if let Some(s) = scheme {
                    dev.rename_scheme(s, &format!("renamed-{a}"));
                }
            }
            11 => {
                if let Some(s) = scheme {
                    dev.reorder_reverse(s);
                }
            }
            12 => {
                if let (Some(s), Some(f)) = (scheme, folder) {
                    dev.move_scheme_to_folder(s, f);
                }
            }
            13 => {
                if let Some(s) = scheme {
                    dev.move_scheme_to_root(s);
                }
            }
            14 => {
                if let Some(s) = scheme {
                    dev.archive_scheme(s);
                }
            }
            15 => {
                if let Some(s) = scheme {
                    dev.restore_scheme(s);
                }
            }
            16 => {
                if let Some(s) = scheme {
                    dev.delete_scheme(s);
                }
            }
            17 => {
                if let Some(nf) = nonroot_folder {
                    dev.rename_folder(nf, &format!("rfolder-{a}"));
                }
            }
            18 => {
                dev.set_daily_queue(date_for(a), &[&format!("dq-{b}")]);
            }
            19 => {
                if let Some(s) = scheme {
                    let n = items_in(s);
                    if n > 0 {
                        dev.attach_image(
                            s,
                            (a as usize) % n,
                            vec![(a % 251) as u8, (b % 251) as u8, 1, 2, 3, 4],
                        );
                    }
                }
            }
            20 => {
                if let Some(s) = scheme {
                    let n = items_in(s);
                    if n > 0 {
                        dev.set_item_indent(s, (a as usize) % n, (b % 4) as u8);
                    }
                }
            }
            21 => {
                if let Some(f) = nonroot_folder {
                    dev.archive_folder(f);
                }
            }
            22 => {
                if let Some(f) = nonroot_folder {
                    dev.restore_folder(f);
                }
            }
            23 => {
                if let Some(f) = nonroot_folder {
                    dev.delete_folder(f);
                }
            }
            24 => {
                if let Some(s) = scheme {
                    let n = items_in(s);
                    if n > 0 {
                        let marker = match b % 4 {
                            0 => ItemMarker::Blank,
                            1 => ItemMarker::Bullet,
                            2 => ItemMarker::Numbered,
                            _ => ItemMarker::Checkbox,
                        };
                        dev.set_item_marker(s, (a as usize) % n, marker);
                    }
                }
            }
            25 => {
                if let Some(s) = scheme {
                    let n = items_in(s);
                    if n > 0 {
                        let start = a.is_multiple_of(2).then(|| {
                            DateTime::<Utc>::from_timestamp(1_800_000_000 + a as i64, 0).unwrap()
                        });
                        let end = b.is_multiple_of(2).then(|| {
                            DateTime::<Utc>::from_timestamp(1_800_100_000 + b as i64, 0).unwrap()
                        });
                        dev.set_item_dates(s, (a as usize) % n, start, end);
                    }
                }
            }
            26 => {
                dev.carryover_daily_queue(date_for(a));
            }
            _ => {}
        }
    }

    /// Revert (undo) or reapply (redo) a device's most recent content snapshot.
    /// At the CRDT layer this is just another local edit, so it must converge
    /// under concurrent edits + syncs like everything else.
    fn undo_or_redo_op(&mut self, i: usize) {
        let redo = self.rng.below(2) == 1;
        self.log(&format!(
            "dev{i} acct{} {}",
            self.devices[i].account,
            if redo { "REDO" } else { "UNDO" }
        ));
        let slot = &mut self.devices[i];
        let popped = if redo {
            slot.redo.pop()
        } else {
            slot.undo.pop()
        };
        let Some((scheme_id, prior_items)) = popped else {
            return;
        };
        // The scheme may have been archived/deleted since the snapshot; only
        // revert content that still exists.
        let Some(current) = slot.dev.scheme_items_snapshot(scheme_id) else {
            return;
        };
        if redo {
            slot.undo.push((scheme_id, current));
        } else {
            slot.redo.push((scheme_id, current));
        }
        slot.dev.revert_scheme_items(scheme_id, prior_items);
    }

    /// Quit and relaunch a device.
    ///
    /// A restart rebuilds the CRDT documents from the *persisted* per-document
    /// states rather than from live memory, and reseeds the local sequence
    /// counter — the layer where several real wedges have lived (a restart
    /// reusing `local_sequence` produced `crdt_schema_invalid`; a rebuilt
    /// document taking a fresh identity orphaned its content). Nothing else in
    /// this fuzzer ever crossed that boundary, so every seed ran as if the app
    /// never closed.
    fn restart_op(&mut self, i: usize) {
        self.log(&format!("dev{i} acct{} RESTART", self.devices[i].account));
        let before = fingerprint(&self.devices[i].dev);
        let pending_before = self.devices[i].dev.pending_count();

        self.devices[i].dev.restart();

        // A relaunch must be invisible: same content, same outbound queue. The
        // wedge invariants after `settle` would eventually catch a divergence,
        // but catching it *here* names the restart as the cause.
        assert_eq!(
            fingerprint(&self.devices[i].dev),
            before,
            "restarting device {i} changed the content it holds"
        );
        assert_eq!(
            self.devices[i].dev.pending_count(),
            pending_before,
            "restarting device {i} lost unpushed edits"
        );
    }

    fn step(&mut self) {
        self.step_no += 1;
        let i = self.rng.below(self.devices.len() as u64) as usize;
        let roll = self.rng.below(100);
        if self.enable_compaction && roll < 4 {
            // The server's nightly at-rest compaction sweep runs on this
            // account. Devices that already pulled must not be forced into a
            // permanent re-pull afterwards.
            let account = self.devices[i].account;
            self.accounts[account].server.run_compaction();
            self.log(&format!("acct{account} SERVER COMPACTION SWEEP"));
        } else if self.enable_restart && roll < 6 {
            self.restart_op(i);
        } else if roll < 25 {
            self.log(&format!("dev{i} acct{} SYNC", self.devices[i].account));
            let _ = self.sync_device(i); // mid-sequence sync errors may self-heal next round
        } else if roll < 34 {
            let from = self.devices[i].account;
            self.switch_account(i);
            self.log(&format!(
                "dev{i} SWITCH acct{from} -> acct{}",
                self.devices[i].account
            ));
        } else if self.enable_undo && roll < 50 {
            // Only the undo fuzz reaches this band; other tests fall straight to
            // `edit_op` with the identical RNG sequence they always had.
            self.undo_or_redo_op(i);
        } else {
            self.edit_op(i);
        }
    }

    fn devices_on(&self, account: usize) -> Vec<usize> {
        (0..self.devices.len())
            .filter(|&i| self.devices[i].account == account)
            .collect()
    }

    fn account_converged(&self, idxs: &[usize]) -> bool {
        match idxs.split_first() {
            Some((&first, rest)) => rest
                .iter()
                .all(|&i| self.devices[first].dev.converges_with(&self.devices[i].dev)),
            None => true,
        }
    }

    /// Sync each account's devices until the devices on it converge (idempotent Yjs
    /// merges mean extra rounds are harmless), bounded so a real divergence can't loop
    /// forever — it surfaces in the assertions instead.
    fn settle(&mut self) {
        for account in 0..self.accounts.len() {
            let idxs = self.devices_on(account);
            if idxs.is_empty() {
                continue;
            }
            let rounds = idxs.len() * 6 + 16;
            for _ in 0..rounds {
                for &i in &idxs {
                    let _ = self.sync_device(i);
                }
                if self.account_converged(&idxs) {
                    for &i in &idxs {
                        let _ = self.sync_device(i);
                    }
                    if self.account_converged(&idxs) {
                        break;
                    }
                }
            }
        }
    }

    fn assert_invariants(&self, seed: u64) {
        for account in 0..self.accounts.len() {
            let idxs = self.devices_on(account);
            if idxs.is_empty() {
                continue;
            }
            // (2) No stuck pending — the wedge symptom — on any device.
            for &i in &idxs {
                assert!(
                    self.devices[i].dev.is_fully_pushed(),
                    "seed {seed}: device {i} on account {account} has stuck pending after settle (wedge)"
                );
            }
            // (3) Devices currently on the same account converge.
            let first = idxs[0];
            for &i in &idxs[1..] {
                if !self.devices[first].dev.converges_with(&self.devices[i].dev) {
                    let fa = fingerprint(&self.devices[first].dev);
                    let fb = fingerprint(&self.devices[i].dev);
                    let only_a: Vec<_> = fa.iter().filter(|x| !fb.contains(x)).collect();
                    let only_b: Vec<_> = fb.iter().filter(|x| !fa.contains(x)).collect();
                    let sa = &self.devices[first].dev.last_skipped;
                    let sb = &self.devices[i].dev.last_skipped;
                    // What does the SERVER actually hold? A fresh puller reveals whether
                    // the missing content was never pushed vs pushed-but-not-applied.
                    let mut puller = fresh_device(self.accounts[account].workspace);
                    for _ in 0..4 {
                        let _ = puller.try_sync(&self.accounts[account].server);
                    }
                    let fp = fingerprint(&puller);
                    let server_has_only_a: Vec<_> =
                        fp.iter().filter(|x| only_a.contains(x)).collect();
                    let diagnostic_scheme = self.devices[first]
                        .dev
                        .workspace
                        .schemes
                        .keys()
                        .copied()
                        .find(|id| only_a.iter().any(|line| line.contains(&id.to_string())))
                        .or_else(|| {
                            self.devices[i]
                                .dev
                                .workspace
                                .schemes
                                .keys()
                                .copied()
                                .find(|id| only_b.iter().any(|line| line.contains(&id.to_string())))
                        });
                    let diagnostic_state = diagnostic_scheme.map(|scheme| {
                        let doc = self.devices[i].dev.scheme_document_id(scheme);
                        let cursor = self.devices[i]
                            .dev
                            .local_state_ref()
                            .document_cursors
                            .get(&doc)
                            .map(|cursor| {
                                (cursor.last_pulled_sequence, cursor.last_pushed_sequence)
                            });
                        (
                            scheme,
                            doc,
                            cursor,
                            self.devices[i].dev.scheme_state_len(scheme),
                        )
                    });
                    panic!(
                        "seed {seed}: devices {first} and {i} on account {account} diverged\n  only on dev{first}: {only_a:#?}\n  only on dev{i}: {only_b:#?}\n  of dev{first}-only, the SERVER has: {server_has_only_a:#?}\n  dev{first} last_skipped: {sa:#?}\n  dev{i} last_skipped: {sb:#?}\n  diagnostic dev{i} scheme/doc/cursor/state: {diagnostic_state:#?}"
                    );
                }
            }
            // (4) A fresh device signing into the account sees exactly the same
            // content — the server holds the full, materializable state.
            let mut puller = fresh_device(self.accounts[account].workspace);
            for _ in 0..3 {
                puller
                    .try_sync(&self.accounts[account].server)
                    .unwrap_or_else(|e| {
                        panic!(
                            "seed {seed}: fresh puller on account {account} failed to sync: {e:#}"
                        )
                    });
            }
            if !puller.converges_with(&self.devices[first].dev) {
                let fa = fingerprint(&self.devices[first].dev);
                let fb = fingerprint(&puller);
                let only_dev: Vec<_> = fa.iter().filter(|x| !fb.contains(x)).collect();
                let only_puller: Vec<_> = fb.iter().filter(|x| !fa.contains(x)).collect();
                panic!(
                    "seed {seed}: a fresh device on account {account} sees different content than existing devices (server-state divergence / silent loss)\n  only on dev{first}: {only_dev:#?}\n  only on fresh puller: {only_puller:#?}"
                );
            }
        }
    }
}

fn run_seed(seed: u64, num_accounts: usize, num_devices: usize, steps: usize) {
    let mut world = World::new(seed, num_accounts, num_devices);
    for _ in 0..steps {
        world.step();
    }
    world.settle();
    world.assert_invariants(seed);
}

/// Cross the persistence boundary deliberately: write schemes, folders, and
/// scheme content into the plain workspace while omitting their CRDT snapshots,
/// then let another device advance the server's workspace index before the
/// damaged device pulls. This is the failure shape ordinary operation fuzzing
/// cannot reach because its helpers author the plain workspace and CRDT together.
fn run_seed_persistence_boundary(seed: u64) {
    let mut world = World::new(seed, 1, 2);
    world.sync_device(0).expect("initial local sync");
    world.sync_device(1).expect("initial peer sync");

    let rounds = 2 + (seed % 4) as usize;
    let mut local_schemes = Vec::new();
    for round in 0..rounds {
        let root = world.devices[0].dev.workspace.root;
        let folder = world.devices[0]
            .dev
            .direct_add_folder_without_crdt(root, &format!("plain-folder-{seed}-{round}"));
        let scheme = world.devices[0].dev.direct_add_scheme_without_crdt(
            folder,
            &format!("plain-scheme-{seed}-{round}"),
            &["plain-content"],
        );
        local_schemes.push(scheme);

        // Advance the server from a different replica, ensuring device 0 must
        // merge a remote workspace-index update while its plain state is ahead
        // of its own CRDT state.
        let peer_root = world.devices[1].dev.workspace.root;
        world.devices[1]
            .dev
            .add_folder(&format!("peer-folder-{seed}-{round}"));
        world.devices[1].dev.add_scheme_to_folder(
            peer_root,
            &format!("peer-scheme-{seed}-{round}"),
            &["peer-content"],
        );
        world
            .sync_device(1)
            .expect("peer sync after local disk write");
        world
            .sync_device(0)
            .expect("plain-only workspace state must survive remote pull");

        // Make the peer learn the repaired scheme, then race a plain-file edit
        // against a peer deletion. The local edit must be re-expressed after
        // the remote tombstone is merged rather than disappearing.
        world
            .sync_device(1)
            .expect("peer learns repaired local scheme");
        world.devices[0].dev.direct_set_line_text_without_crdt(
            scheme,
            0,
            &format!("plain-repair-{seed}-{round}"),
        );
        world.devices[1].dev.remove_line(scheme, 0);
        world.sync_device(1).expect("peer deletion sync");
        world
            .sync_device(0)
            .expect("plain-only edit must survive remote tombstone");

        // Repeat the mismatch with an existing scheme's content, not only a
        // missing document, then force another remote index change before pull.
        world.devices[0]
            .dev
            .direct_append_line_without_crdt(scheme, &format!("plain-edit-{seed}-{round}"));
        world.devices[1]
            .dev
            .add_folder(&format!("peer-folder-2-{seed}-{round}"));
        world.sync_device(1).expect("peer index advancement");
        world
            .sync_device(0)
            .expect("plain-only existing content must survive remote pull");
    }

    world.settle();
    world.assert_invariants(seed);
    for scheme in local_schemes {
        let Some(local) = world.devices[0].dev.workspace.schemes.get(&scheme) else {
            panic!("seed {seed}: persistence fuzz lost local scheme {scheme}");
        };
        assert!(
            local.items.iter().any(|item| {
                item.text() == "plain-content" || item.text().starts_with("plain-repair-")
            }),
            "seed {seed}: persistence fuzz lost local scheme content"
        );
    }
}

/// A transport that keeps returning the same valid scheme snapshot even after
/// the client advances its cursor. This models the old pull-loop wedge: the
/// workspace index binds a scheme, but the local CRDT cannot materialize it, so
/// resetting the cursor only replays the same page forever. The transport stops
/// after a few calls so the old implementation fails quickly instead of hanging
/// the test process.
struct RepeatingMaterializationGapTransport {
    pull_calls: Cell<usize>,
    document: DocumentId,
    state_v1: Vec<u8>,
}

impl SyncTransport for RepeatingMaterializationGapTransport {
    fn pull(&self, _request: &BatchPullRequest) -> anyhow::Result<BatchPullResponse> {
        let call = self.pull_calls.get() + 1;
        self.pull_calls.set(call);
        if call > 4 {
            return Err(anyhow::anyhow!(
                "materialization-gap fuzz case did not terminate"
            ));
        }
        Ok(BatchPullResponse {
            documents: vec![PulledCrdtDocument {
                document: self.document,
                kind: SyncDocumentKind::Scheme,
                seq: 1,
                epoch: 0,
                state_v1: self.state_v1.clone(),
                state_v1_is_delta: false,
            }],
            known_documents: Some(HashMap::from([(self.document, 1)])),
            ..BatchPullResponse::default()
        })
    }

    fn push(&self, _request: &BatchPushRequest) -> anyhow::Result<BatchPushResponse> {
        Ok(BatchPushResponse::default())
    }
}

/// Seeded coverage for the exact materialization-gap shape. The ordinary model
/// fuzzer exercises the real server and device lifecycle; this smaller sweep
/// deliberately supplies the pathological repeated page that is otherwise rare
/// in random operation sequences, and proves every seed remains bounded.
#[test]
fn materialization_gap_fuzz_is_bounded() {
    let seeds = env_usize("KNOTQ_FUZZ_SEEDS", 24);
    run_seeds_parallel(seeds, |seed| {
        let mut indexed_workspace = Workspace::new();
        let mut scheme = knotq_model::Scheme::new(format!("Older scheme {seed}"), 0);
        for item in 0..=seed % 3 {
            scheme
                .items
                .push(Item::new(format!("remote-{seed}-{item}")));
        }
        let scheme_id = scheme.id;
        indexed_workspace.schemes.insert(scheme_id, scheme);
        indexed_workspace.ensure_sync_metadata();
        let document = indexed_workspace.scheme_sync[&scheme_id].id;
        let server_state = WorkspaceCrdtDocuments::try_new(&indexed_workspace)
            .unwrap()
            .document_states()[&document]
            .to_vec();

        // Keep the durable binding but remove the materialized scheme body.
        let mut stale_workspace = indexed_workspace;
        stale_workspace.schemes.remove(&scheme_id);
        let mut local_crdt = WorkspaceCrdtDocuments::try_new(&stale_workspace).unwrap();
        let transport = RepeatingMaterializationGapTransport {
            pull_calls: Cell::new(0),
            document,
            state_v1: server_state,
        };
        let mut local_state = knotq_sync::LocalSyncState::default();

        let outcome = batch_pull_and_apply(
            &transport,
            &mut local_crdt,
            &mut local_state,
            stale_workspace,
            ReplicaId::new(),
        )
        .unwrap_or_else(|error| panic!("seed {seed}: materialization gap wedged: {error:#}"));

        assert_eq!(
            outcome.pull_requests, 1,
            "seed {seed}: repeated materialization gap should finish in one pull"
        );
        assert_eq!(transport.pull_calls.get(), 1);
        assert_eq!(
            local_state.document_cursors[&document].last_pulled_sequence, 1,
            "seed {seed}: cursor must advance past the unmaterializable page"
        );
    });
}

/// Undo, redo *and* restarts in the same world — the combination that is hardest
/// to reason about, since a restart drops the in-memory CRDT documents while a
/// device still has an undo stack and unpushed edits.
fn run_seed_restart(seed: u64, num_accounts: usize, num_devices: usize, steps: usize) {
    let mut world = World::new(seed, num_accounts, num_devices);
    world.enable_undo = true;
    world.enable_restart = true;
    for _ in 0..steps {
        world.step();
    }
    world.settle();
    world.assert_invariants(seed);
}

/// The server periodically runs its at-rest compaction sweep (v1->v2->v1
/// transcode + re-encode of every stored document, seq unchanged) while devices
/// keep editing and syncing. Guards against a compaction that is not
/// state-vector-preserving, which would wedge every device that pulled a
/// document before the sweep on the pull integrity check forever.
fn run_seed_compaction(seed: u64, num_accounts: usize, num_devices: usize, steps: usize) {
    let mut world = World::new(seed, num_accounts, num_devices);
    world.enable_compaction = true;
    world.enable_restart = true;
    for _ in 0..steps {
        world.step();
    }
    // A final sweep after all edits, so the very last state each device holds is
    // also checked against a freshly-compacted server.
    for account in 0..world.accounts.len() {
        world.accounts[account].server.run_compaction();
    }
    world.settle();
    world.assert_invariants(seed);

    // A caught-up device must not be told to re-pull anything by the pull
    // integrity check. A non-zero count `settle` could not clear is the "stuck
    // on Resyncing" livelock — most plausibly a server rewrite (compaction) that
    // changed a document's state vector out from under a device that had already
    // pulled it.
    for account in 0..world.accounts.len() {
        let idxs = world.devices_on(account);
        let Some(&first) = idxs.first() else { continue };
        world.accounts[account].server.run_compaction();
        let _ = world.devices[first]
            .dev
            .try_sync(&world.accounts[account].server);
        // The compaction-specific invariant: for every document the caught-up
        // device DID hold and submit a vector for, the server's re-derived
        // vector must match. A disagreement here means the v1->v2->v1 transcode
        // (or the re-encode) changed a document's state vector, which would wedge
        // that device on the pull integrity check forever.
        //
        // A *missing* vector (`last_integrity_mismatch_count` > its disagreement
        // subset) is a different problem — the server tracks a document the
        // client cannot materialize (an orphaned `scheme_sync` binding). That is
        // covered separately; it is not caused by compaction.
        assert_eq!(
            world.accounts[account]
                .server
                .last_integrity_vector_disagreement_count(),
            0,
            "seed {seed}: account {account}: a caught-up device's state vector disagrees with \
             the server's re-derived one after compaction — the compaction sweep is not \
             state-vector-preserving, which wedges that device on the pull integrity check"
        );
    }
}

fn run_seed_undo(seed: u64, num_accounts: usize, num_devices: usize, steps: usize) {
    let mut world = World::new(seed, num_accounts, num_devices);
    world.enable_undo = true;
    for _ in 0..steps {
        world.step();
    }
    world.settle();
    world.assert_invariants(seed);
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Run independent seeds across a bounded worker pool. The deterministic model
/// seed is thread-local, so each worker gets a reproducible UUID stream and a
/// failure still reports the exact seed to replay. Keep the default small
/// because libtest may run several fuzz tests at once; set
/// `KNOTQ_FUZZ_WORKERS=1` for serial behavior or raise it on a dedicated host.
fn run_seeds_parallel(seeds: usize, run: impl Fn(u64) + Sync) {
    if seeds == 0 {
        return;
    }
    let default_workers = std::thread::available_parallelism()
        .map(|parallelism| parallelism.get().min(4))
        .unwrap_or(4);
    let workers = env_usize("KNOTQ_FUZZ_WORKERS", default_workers)
        .max(1)
        .min(seeds);
    let next_seed = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(workers);
        for _ in 0..workers {
            handles.push(scope.spawn(|| loop {
                let seed = next_seed.fetch_add(1, Ordering::Relaxed);
                if seed >= seeds {
                    break;
                }
                run(seed as u64);
            }));
        }
        for handle in handles {
            handle.join().expect("sync fuzzer worker panicked");
        }
    });
}

/// The main fuzz: 3 accounts, 4 devices, lots of mixed operations including account
/// switches, over many seeds. Override breadth/depth with env vars for deep runs:
///   KNOTQ_FUZZ_SEEDS=2000 KNOTQ_FUZZ_STEPS=400 cargo test -p knotq-sync --test sync_property_model -- --nocapture
#[test]
fn multi_account_fuzz_converges() {
    let seeds = env_usize("KNOTQ_FUZZ_SEEDS", 24);
    let steps = env_usize("KNOTQ_FUZZ_STEPS", 140);
    run_seeds_parallel(seeds, |seed| {
        run_seed(seed, 3, 4, steps);
    });
}

/// Persistence-boundary fuzz: every seed includes direct plain-workspace
/// creation of folders/schemes and direct edits to existing scheme content,
/// with a peer advancing the remote index between each local write and pull.
#[test]
fn persistence_boundary_fuzz_converges() {
    let default_seeds = env_usize("KNOTQ_FUZZ_SEEDS", 32);
    let seeds = env_usize("KNOTQ_PERSISTENCE_FUZZ_SEEDS", default_seeds);
    run_seeds_parallel(seeds, |seed| {
        run_seed_persistence_boundary(seed.wrapping_add(0x9e37_79b9));
    });
}

/// Switch-heavy variant: 2 devices that hop between 4 accounts constantly, stressing
/// the sign-out/sign-in cursor reset and workspace re-identify paths specifically.
#[test]
fn account_hopping_fuzz_converges() {
    let seeds = env_usize("KNOTQ_FUZZ_SEEDS", 24);
    run_seeds_parallel(seeds, |seed| {
        // Bias toward switches by interleaving extra switch+sync after each seed's run
        // is handled inside run_seed via the op weights; here we just widen accounts.
        run_seed(seed.wrapping_mul(2_654_435_761), 4, 2, 120);
    });
}

/// Regression for the multi-origin daily-queue "carryover" divergence. A reused stable
/// clientID aliased two operations onto one `(clientID, clock)`, making the Yjs merge
/// non-commutative — the server (base-then-push) and a device (local-then-pull) landed
/// on different sides and never reconverged, so a device's tombstones were lost. Both
/// seeds diverged before the all-random-clientID fix; they must converge now. Runs
/// unconditionally so the fix cannot silently regress.
#[test]
fn daily_queue_carryover_merge_regression() {
    run_seed(111486301962, 4, 2, 120); // account-hopping case
    run_seed(421, 3, 4, 140); // multi-account, surfaced at 600-seed depth
}

#[test]
#[ignore]
fn daily_queue_multiorigin_stress() {
    let date = NaiveDate::from_ymd_opt(2026, 7, 15).unwrap();
    let sid = knotq_model::daily_queue_scheme_id(date);
    let seeds = env_usize("KNOTQ_FUZZ_SEEDS", 3000) as u64;
    for seed in 0..seeds {
        knotq_model::set_deterministic_id_seed(Some(seed));
        let account = WorkspaceId::new();
        let server = TestServer::default();
        let n = 3 + (seed % 2) as usize; // 3 or 4 devices
        let mut devs: Vec<TestDevice> = (0..n).map(|_| fresh_device(account)).collect();
        // All create the same day offline first → independent origins of one document.
        for (i, d) in devs.iter_mut().enumerate() {
            d.set_daily_queue(date, &[&format!("init-{i}")]);
        }
        // Concurrent appends with interleaved partial syncs.
        for round in 0..5u64 {
            for (i, dev) in devs.iter_mut().enumerate() {
                dev.append_line(sid, &format!("r{round}d{i}"));
                if (seed + round + i as u64).is_multiple_of(2) {
                    let _ = dev.try_sync(&server);
                }
            }
        }
        // Settle.
        for _ in 0..(n * 6 + 16) {
            for d in devs.iter_mut() {
                let _ = d.try_sync(&server);
            }
        }
        // Convergence among devices + a fresh puller (server-state) must match.
        let mut puller = fresh_device(account);
        for _ in 0..4 {
            let _ = puller.try_sync(&server);
        }
        for i in 1..n {
            assert!(
                devs[0].converges_with(&devs[i]),
                "seed {seed} (n={n}): dev0 vs dev{i} diverged\n  dev0:  {:?}\n  dev{i}: {:?}\n  server: {:?}",
                fingerprint(&devs[0]),
                fingerprint(&devs[i]),
                fingerprint(&puller)
            );
        }
        assert!(
            puller.converges_with(&devs[0]),
            "seed {seed} (n={n}): fresh puller (server) diverged from dev0\n  dev0:   {:?}\n  server: {:?}",
            fingerprint(&devs[0]),
            fingerprint(&puller)
        );
    }
}

#[test]
fn single_account_many_devices_fuzz_converges() {
    let seeds = env_usize("KNOTQ_FUZZ_SEEDS", 24);
    run_seeds_parallel(seeds, |seed| {
        run_seed(seed.wrapping_add(7), 1, 5, 160);
    });
}

/// Undo/redo fuzz: the same multi-device world, but devices also revert scheme
/// content to prior snapshots (undo) and reapply them (redo) mid-stream, racing
/// concurrent edits and syncs from other devices. An undo is just another local
/// edit at the CRDT layer, so every invariant must still hold — this guards
/// against a content revert wedging a device or diverging sync.
#[test]
fn undo_redo_fuzz_converges() {
    let seeds = env_usize("KNOTQ_FUZZ_SEEDS", 24);
    let steps = env_usize("KNOTQ_FUZZ_STEPS", 160);
    run_seeds_parallel(seeds, |seed| {
        run_seed_undo(seed.wrapping_add(13), 3, 4, steps);
    });
}

/// At-rest compaction fuzz: the server rewrites every stored document (v1->v2->v1
/// transcode + materialized re-encode, seq unchanged) while devices edit, sync,
/// and restart. Asserts convergence AND that a caught-up device is never left
/// re-pulling by the pull integrity check — i.e. the compaction is
/// state-vector-preserving in every interleaving.
#[test]
fn compaction_sweep_fuzz_converges() {
    let seeds = env_usize("KNOTQ_FUZZ_SEEDS", 24);
    let steps = env_usize("KNOTQ_FUZZ_STEPS", 160);
    run_seeds_parallel(seeds, |seed| {
        run_seed_compaction(seed.wrapping_add(101), 2, 4, steps);
    });
}

/// Restart fuzz: devices quit and relaunch mid-stream, restoring their CRDT
/// documents from the persisted per-document states while edits, undos, account
/// switches and syncs continue around them.
///
/// This is the one boundary the other fuzz tests never cross, and it is where
/// the sequence-reuse and document-identity wedges lived.
#[test]
fn restart_fuzz_converges() {
    let seeds = env_usize("KNOTQ_FUZZ_SEEDS", 24);
    let steps = env_usize("KNOTQ_FUZZ_STEPS", 180);
    run_seeds_parallel(seeds, |seed| {
        run_seed_restart(seed.wrapping_add(101), 3, 4, steps);
    });
}

/// A device that restarts while it still has unpushed edits must push exactly
/// those edits — not re-mint their sequence numbers, and not drop them.
#[test]
fn restarting_with_unpushed_edits_keeps_them_pushable() {
    let seeds = env_usize("KNOTQ_FUZZ_SEEDS", 16);
    run_seeds_parallel(seeds, |seed| {
        // One account, two devices: every edit one device makes must reach the
        // other, restart or not.
        run_seed_restart(seed.wrapping_mul(6_364_136_223_846_793_005), 1, 2, 140);
    });
}

#[test]
fn account_switch_reseed_and_noop_materialization_regression_seed_127() {
    run_seed_undo(127, 3, 4, 300);
}

#[test]
fn account_switch_self_heal_and_convergence_regression_seed_33() {
    run_seed(33, 3, 4, 300);
}

/// Replay a single seed of the undo fuzz, for triaging a failure a broad run
/// reported.
///
/// A broad run's failing seed reproduces here directly: CRDT clientIDs,
/// folder->document bindings, document creation order and fractional position
/// minting are all deterministic under `set_deterministic_id_seed`, so a seed
/// that took ten minutes to find replays in well under a second. If a future
/// change reintroduces unseeded randomness on the sync path that stops being
/// true — which is what `client_id_determinism.rs` guards.
#[test]
#[ignore = "triage helper; runs one seed chosen by KNOTQ_REPRO_SEED"]
fn replay_undo_seed() {
    let seed = env_usize("KNOTQ_REPRO_SEED", 349) as u64;
    let steps = env_usize("KNOTQ_FUZZ_STEPS", 400);
    run_seed_undo(seed.wrapping_add(13), 3, 4, steps);
}

/// Pinned regression for the account-switch push corruption.
///
/// This exact scenario wedged a device's push loop with repeated
/// `crdt_schema_invalid` rejections: after hopping accounts it pushed an
/// incremental delta for a *derived* document id whose base on the new server
/// shared none of its history. It took 1500 seeds x 400 steps of
/// `account_hopping_fuzz_converges` to surface (~10 minutes), far too slow to
/// guard every change — but the run is now deterministic, so the one failing
/// seed replays here in milliseconds and CI gets the guard for free.
///
/// Reproducibility rests on clientIDs, folder->document bindings, document
/// creation order and fractional position minting all being deterministic under
/// `set_deterministic_id_seed`; if a future change reintroduces unseeded
/// randomness on the sync path this stops testing what it claims to, which is
/// what `client_id_determinism.rs` guards.
#[test]
fn account_switch_push_corruption_regression() {
    run_seed(3_965_727_026_934, 4, 2, 120);
}

/// Pinned regression for the frozen-stale-workspace divergence.
///
/// A device's CRDT document and the server agreed while its materialized
/// workspace showed older content — permanently. `materialize_workspace` reused
/// `current`'s items for any scheme the current batch did not change, assuming
/// such a document matches what produced `current`; a document that took content
/// in a merge whose result was never adopted breaks that assumption, and since
/// Yjs replays a delivered update as a no-op the scheme is never marked changed
/// again. Took 3000 seeds x 400 steps of `undo_redo_fuzz_converges` to surface;
/// replays here in under a second.
#[test]
fn frozen_stale_workspace_divergence_regression() {
    run_seed_undo(2_336, 3, 4, 400);
}

/// Replay a single seed of the plain (non-undo) fuzz, for triaging a failure a
/// broad run reported. `KNOTQ_REPRO_ACCOUNTS` / `KNOTQ_REPRO_DEVICES` /
/// `KNOTQ_FUZZ_STEPS` match the shape the reporting test used.
#[test]
#[ignore = "triage helper; runs one seed chosen by KNOTQ_REPRO_SEED"]
fn replay_seed() {
    let seed: u64 = std::env::var("KNOTQ_REPRO_SEED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let accounts = env_usize("KNOTQ_REPRO_ACCOUNTS", 4);
    let devices = env_usize("KNOTQ_REPRO_DEVICES", 2);
    let steps = env_usize("KNOTQ_FUZZ_STEPS", 120);
    run_seed(seed, accounts, devices, steps);
}

#[test]
#[ignore = "triage helper; runs one restart seed chosen by KNOTQ_REPRO_SEED"]
fn replay_restart_seed() {
    let seed = env_usize("KNOTQ_REPRO_SEED", 585) as u64;
    let steps = env_usize("KNOTQ_FUZZ_STEPS", 400);
    run_seed_restart(seed, 3, 4, steps);
}

#[test]
#[ignore = "triage helper; runs one compaction seed chosen by KNOTQ_REPRO_SEED"]
fn replay_compaction_seed() {
    let seed = env_usize("KNOTQ_REPRO_SEED", 527) as u64;
    let steps = env_usize("KNOTQ_FUZZ_STEPS", 400);
    run_seed_compaction(seed, 2, 4, steps);
}
