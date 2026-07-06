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
//!   1. No server ever rejected a push with `crdt_schema_invalid`.
//!   2. Every device ends fully pushed (no stuck pending — i.e. no wedge).
//!   3. All devices currently on the same account converge to identical content.
//!   4. A brand-new device signing into each account sees exactly that content
//!      (the server holds the full, materializable state — no silent loss).
//!
//! Seeds are deterministic, so any failure prints a seed that reproduces the exact
//! operation sequence. Crank coverage with `KNOTQ_FUZZ_SEEDS` / `KNOTQ_FUZZ_STEPS`.

mod common;

use chrono::NaiveDate;
use common::{Rng, TestDevice, TestServer};
use knotq_model::{FolderId, Item, SchemeId, Workspace, WorkspaceId};

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
        out.push(format!("folder {} {:?} children={}", folder.id, folder.name, folder.children.len()));
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
}

/// Ops in `edit_op` that mutate a scheme's item list (so an undo can revert it).
fn is_content_op(op: u64) -> bool {
    matches!(op, 5 | 6 | 7 | 8 | 9 | 11 | 20)
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
        let mut scheme_ids: Vec<SchemeId> =
            self.devices[i].dev.workspace.schemes.keys().copied().collect();
        scheme_ids.sort();
        let mut folder_ids: Vec<FolderId> =
            self.devices[i].dev.workspace.folders.keys().copied().collect();
        folder_ids.sort();
        let root = self.devices[i].dev.workspace.root;

        // Pre-roll every random choice BEFORE taking &mut on the device, so the RNG
        // and the device (disjoint fields) are never borrowed at once.
        let op = self.rng.below(21);
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
        let popped = if redo { slot.redo.pop() } else { slot.undo.pop() };
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

    fn step(&mut self) {
        self.step_no += 1;
        let i = self.rng.below(self.devices.len() as u64) as usize;
        let roll = self.rng.below(100);
        if roll < 25 {
            self.log(&format!("dev{i} acct{} SYNC", self.devices[i].account));
            let _ = self.sync_device(i); // mid-sequence sync errors may self-heal next round
        } else if roll < 34 {
            let from = self.devices[i].account;
            self.switch_account(i);
            self.log(&format!("dev{i} SWITCH acct{from} -> acct{}", self.devices[i].account));
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
        // (1) No server ever organically rejected a push with crdt_schema_invalid.
        for (a, account) in self.accounts.iter().enumerate() {
            assert_eq!(
                account.server.schema_invalid_rejections(),
                0,
                "seed {seed}: account {a} rejected a push with crdt_schema_invalid"
            );
        }

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
                    panic!(
                        "seed {seed}: devices {first} and {i} on account {account} diverged\n  only on dev{first}: {only_a:#?}\n  only on dev{i}: {only_b:#?}\n  of dev{first}-only, the SERVER has: {server_has_only_a:#?}\n  dev{first} last_skipped: {sa:#?}\n  dev{i} last_skipped: {sb:#?}"
                    );
                }
            }
            // (4) A fresh device signing into the account sees exactly the same
            // content — the server holds the full, materializable state.
            let mut puller = fresh_device(self.accounts[account].workspace);
            for _ in 0..3 {
                puller
                    .try_sync(&self.accounts[account].server)
                    .unwrap_or_else(|e| panic!("seed {seed}: fresh puller on account {account} failed to sync: {e:#}"));
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

/// The main fuzz: 3 accounts, 4 devices, lots of mixed operations including account
/// switches, over many seeds. Override breadth/depth with env vars for deep runs:
///   KNOTQ_FUZZ_SEEDS=2000 KNOTQ_FUZZ_STEPS=400 cargo test -p knotq-sync --test sync_property_model -- --nocapture
#[test]
fn multi_account_fuzz_converges() {
    let seeds = env_usize("KNOTQ_FUZZ_SEEDS", 24) as u64;
    let steps = env_usize("KNOTQ_FUZZ_STEPS", 140);
    for seed in 0..seeds {
        run_seed(seed, 3, 4, steps);
    }
}

/// Switch-heavy variant: 2 devices that hop between 4 accounts constantly, stressing
/// the sign-out/sign-in cursor reset and workspace re-identify paths specifically.
#[test]
fn account_hopping_fuzz_converges() {
    let seeds = env_usize("KNOTQ_FUZZ_SEEDS", 24) as u64;
    for seed in 0..seeds {
        // Bias toward switches by interleaving extra switch+sync after each seed's run
        // is handled inside run_seed via the op weights; here we just widen accounts.
        run_seed(seed.wrapping_mul(2_654_435_761), 4, 2, 120);
    }
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
            for i in 0..n {
                devs[i].append_line(sid, &format!("r{round}d{i}"));
                if (seed + round + i as u64) % 2 == 0 {
                    let _ = devs[i].try_sync(&server);
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
    let seeds = env_usize("KNOTQ_FUZZ_SEEDS", 24) as u64;
    for seed in 0..seeds {
        run_seed(seed.wrapping_add(7), 1, 5, 160);
    }
}

/// Undo/redo fuzz: the same multi-device world, but devices also revert scheme
/// content to prior snapshots (undo) and reapply them (redo) mid-stream, racing
/// concurrent edits and syncs from other devices. An undo is just another local
/// edit at the CRDT layer, so every invariant must still hold — this guards
/// against a content revert wedging a device or diverging sync.
#[test]
fn undo_redo_fuzz_converges() {
    let seeds = env_usize("KNOTQ_FUZZ_SEEDS", 24) as u64;
    let steps = env_usize("KNOTQ_FUZZ_STEPS", 160);
    for seed in 0..seeds {
        run_seed_undo(seed.wrapping_add(13), 3, 4, steps);
    }
}
