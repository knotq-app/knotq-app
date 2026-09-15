//! Production-path sync fuzzer for the desktop app.
//!
//! Every simulated device is the real thing below the UI: `AppState` and its
//! store, commands applied through the state layer, the app's startup load,
//! save task, and shutdown flush against a real on-disk data directory, and the
//! real `sync_snapshot` plus the sync task's landing (including edits applied
//! while a run is in flight) against an in-memory backend per account.
//!
//! On top of convergence it checks the no-silent-loss oracle (`oracle.rs`) after
//! every sync and relaunch, so a loss every device agrees on still fails.
//!
//! `KNOTQ_FUZZ_SEEDS` / `KNOTQ_FUZZ_STEPS` widen a run; a failure prints the
//! seed, and `KNOTQ_REPRO_SEED=<n> cargo test -p knotq-app replay_production_seed
//! -- --ignored --nocapture` replays it. `KNOTQ_FUZZ_TRACE=1` prints every step.

mod actions;
mod backend;
mod device;
mod oracle;
mod scenarios;

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::NaiveDate;

use backend::Account;
use device::{CrashPoint, DesktopDevice};
use oracle::{diff_lines, Attribution, View};

pub(super) struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed ^ 0xD1B5_4A32_D192_ED03)
    }

    pub(super) fn below(&mut self, bound: u64) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        if bound == 0 {
            0
        } else {
            z % bound
        }
    }
}

struct Config {
    accounts: usize,
    initial_devices: usize,
    max_devices: usize,
    steps: usize,
    /// Account switching, faults, crashes — off for the plainest single-account run.
    chaos: bool,
}

struct World {
    seed: u64,
    root: PathBuf,
    accounts: Vec<Account>,
    /// What each account's server materializes, as of the last sync against it.
    server_views: Vec<View>,
    devices: Vec<Option<DesktopDevice>>,
    rng: Rng,
    attribution: Attribution,
    violations: Vec<String>,
    trace: bool,
    step: usize,
    config: Config,
}

fn fuzz_root(seed: u64) -> PathBuf {
    static RUN: AtomicUsize = AtomicUsize::new(0);
    let run = RUN.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "knotq-production-fuzz-{}-{seed}-{run}",
        std::process::id()
    ))
}

impl World {
    fn new(seed: u64, config: Config) -> Self {
        knotq_model::set_deterministic_id_seed(Some(seed));
        let root = fuzz_root(seed);
        let _ = std::fs::remove_dir_all(&root);
        let mut world = Self {
            seed,
            root,
            accounts: (0..config.accounts).map(Account::new).collect(),
            server_views: (0..config.accounts).map(|_| View::default()).collect(),
            devices: Vec::new(),
            rng: Rng::new(seed),
            attribution: Attribution::default(),
            violations: Vec::new(),
            trace: std::env::var("KNOTQ_FUZZ_TRACE").is_ok(),
            step: 0,
            config,
        };
        for _ in 0..world.config.initial_devices {
            let account = world.rng.below(world.accounts.len() as u64) as usize;
            world.add_device(Some(account));
        }
        world
    }

    fn today() -> NaiveDate {
        chrono::Local::now().date_naive()
    }

    fn log(&self, message: impl AsRef<str>) {
        if self.trace {
            eprintln!(
                "[seed {} step {:>4}] {}",
                self.seed,
                self.step,
                message.as_ref()
            );
        }
    }

    fn add_device(&mut self, account: Option<usize>) -> usize {
        let index = self.devices.len();
        let dir = self.root.join(format!("device-{index}"));
        let mut device = DesktopDevice::install(index, dir, Self::today());
        let seeded = View::of(&device.full_workspace());
        self.attribution.record_seed(index, &seeded);
        if let Some(account) = account {
            device.sign_in(&self.accounts[account]);
        }
        self.devices.push(Some(device));
        self.log(format!("device {index} installed, account {account:?}"));
        index
    }

    fn live_devices(&self) -> Vec<usize> {
        (0..self.devices.len())
            .filter(|index| self.devices[*index].is_some())
            .collect()
    }

    fn view(&self, index: usize) -> View {
        View::of(&self.devices[index].as_ref().unwrap().full_workspace())
    }

    /// Run `action` as a local step on device `index` and attribute its effect.
    fn local<R>(
        &mut self,
        index: usize,
        action: impl FnOnce(&mut DesktopDevice, &mut Rng) -> R,
    ) -> R {
        let before = self.view(index);
        let device = self.devices[index].as_mut().unwrap();
        let result = action(device, &mut self.rng);
        let after = self.view(index);
        self.attribution.record_local(index, &before, &after);
        result
    }

    fn check(&mut self, index: usize, label: &str, before: &View, after: &View) {
        let found = self
            .attribution
            .check_passive(index, label, before, after, true);
        for violation in found {
            self.log(format!("VIOLATION {violation}"));
            self.violations
                .push(format!("step {}: {violation}", self.step));
        }
    }

    /// A full sync attempt, optionally with local edits landing while the run
    /// is in flight.
    fn sync(&mut self, index: usize, in_flight_edits: usize) {
        let Some(account) = self.devices[index].as_ref().unwrap().account else {
            return;
        };
        let allow_squash = self.rng.below(4) == 0;
        let run = {
            let device = self.devices[index].as_mut().unwrap();
            device.run_sync(&self.accounts[account], allow_squash)
        };
        let Some(run) = run else { return };
        self.log(format!("device {index} run: {}", run.summary()));
        for _ in 0..in_flight_edits {
            let label = self.local(index, actions::random_local_action);
            self.log(format!("device {index} in-flight: {label}"));
        }
        // The save task can run between a run's own save and its landing,
        // writing the store's pre-landing workspace over the files the run
        // just wrote.
        if self.rng.below(4) == 0 {
            let _ = self.devices[index].as_mut().unwrap().save();
            self.log(format!("device {index} in-flight: saved"));
        }
        // The app can quit or crash before a finished run lands: the run's
        // pushes happened, but its result is never adopted.
        if self.config.chaos && self.rng.below(10) == 0 {
            drop(run);
            if self.rng.below(2) == 0 {
                self.log(format!("device {index} in-flight: crashed before landing"));
                self.crash(index);
            } else {
                // The user quits (or an update restarts the app) before the run
                // lands; the flush writes the older store, so the run's pulls
                // have to come in again.
                self.log(format!("device {index} in-flight: quit before landing"));
                self.relaunch(index);
            }
            self.audit_server(account, index);
            return;
        }
        let before = self.view(index);
        let error = self.devices[index].as_mut().unwrap().land_sync(run);
        let epoch_stale = error.as_ref().is_some_and(|err| {
            err.downcast_ref::<knotq_sync::SyncPushEpochStale>()
                .is_some()
        });
        if epoch_stale {
            // The scheduler re-runs the attempt once.
            let device = self.devices[index].as_mut().unwrap();
            let _ = device.sync_now(&self.accounts[account], false);
        }
        let after = self.view(index);
        self.log(format!(
            "device {index} synced account {account}: {}",
            error.map_or("ok".to_string(), |err| format!("{err:#}"))
        ));
        self.check(index, "sync", &before, &after);
        self.audit_server(account, index);
    }

    /// What a brand-new device signing into `account` would materialize.
    fn server_view(&self, account: usize) -> View {
        let account = &self.accounts[account];
        let mut base = knotq_model::Workspace::new();
        base.canonicalize_personal_sync_identity(account.workspace);
        let replica = knotq_model::ReplicaId::new();
        let mut crdt = knotq_sync::WorkspaceCrdtDocuments::from_states::<Vec<u8>>(
            &base,
            replica,
            &std::collections::HashMap::new(),
        )
        .expect("empty audit CRDT");
        let mut local_state = knotq_sync::LocalSyncState::default();
        match knotq_sync::batch_pull_and_apply(
            &account.server.audit(),
            &mut crdt,
            &mut local_state,
            base,
            replica,
        ) {
            Ok(outcome) => View::of(&outcome.workspace),
            Err(err) => {
                self.log(format!("server audit pull failed: {err:#}"));
                self.server_views[account.index].clone()
            }
        }
    }

    /// The server must never lose something no device deleted. Checked after
    /// every sync, so a violation names the device whose push caused it.
    fn audit_server(&mut self, account: usize, pusher: usize) {
        let after = self.server_view(account);
        let label = format!("device {pusher}'s sync (server state)");
        let found = self.attribution.check_passive(
            usize::MAX,
            &label,
            &self.server_views[account],
            &after,
            false,
        );
        for violation in found {
            self.log(format!("VIOLATION account {account} {violation}"));
            self.violations
                .push(format!("step {}: account {account} {violation}", self.step));
        }
        self.server_views[account] = after;
    }

    fn relaunch(&mut self, index: usize) {
        // Shutdown completes elapsed events — a local edit, attributed as one.
        self.local(index, |device, _| {
            knotq_state::complete_past_events(&mut device.state, chrono::Utc::now());
        });
        let before = self.view(index);
        let device = self.devices[index].take().unwrap();
        self.devices[index] = Some(device.relaunch());
        let after = self.view(index);
        self.log(format!("device {index} quit and relaunched"));
        self.check(index, "relaunch", &before, &after);
    }

    fn crash(&mut self, index: usize) {
        let point = match self.rng.below(3) {
            0 => CrashPoint::BeforeSave,
            1 => CrashPoint::AfterWorkspace,
            _ => CrashPoint::AfterPending,
        };
        let device = self.devices[index].take().unwrap();
        self.devices[index] = Some(device.crash(point));
        // Unsaved work is legitimately gone; whatever the device shows now is
        // what later steps must preserve.
        self.log(format!("device {index} crashed at {point:?}"));
    }

    fn step(&mut self) {
        self.step += 1;
        let live = self.live_devices();
        let index = live[self.rng.below(live.len() as u64) as usize];
        let roll = self.rng.below(100);
        let chaos = self.config.chaos;
        match roll {
            0..=44 => {
                let label = self.local(index, actions::random_local_action);
                self.log(format!("device {index}: {label}"));
            }
            45..=62 => {
                let in_flight = if self.rng.below(4) == 0 {
                    1 + self.rng.below(3) as usize
                } else {
                    0
                };
                self.sync(index, in_flight);
            }
            63..=69 => {
                let _ = self.devices[index].as_mut().unwrap().save();
                self.log(format!("device {index} saved"));
            }
            70..=74 => self.relaunch(index),
            75..=77 if chaos => self.crash(index),
            78..=81 if chaos && self.accounts.len() > 1 => {
                let target = self.rng.below(self.accounts.len() as u64) as usize;
                let device = self.devices[index].as_mut().unwrap();
                device.sign_in(&self.accounts[target]);
                self.log(format!("device {index} signed into account {target}"));
            }
            82 if chaos => {
                self.devices[index].as_mut().unwrap().sign_out();
                self.log(format!("device {index} signed out"));
            }
            83..=88 if chaos => {
                let account = self.rng.below(self.accounts.len() as u64) as usize;
                let server = &self.accounts[account].server;
                match self.rng.below(4) {
                    0 => server.fail_next_pulls(1),
                    1 => server.lose_next_push_responses(1),
                    2 => server.reject_next_push_with_schema_invalid(),
                    _ => server.run_compaction(),
                }
                self.log(format!("account {account}: server fault injected"));
            }
            89..=90 if self.devices.len() < self.config.max_devices => {
                let account = self.rng.below(self.accounts.len() as u64) as usize;
                self.add_device(Some(account));
            }
            _ => {
                let label = self.local(index, actions::random_local_action);
                self.log(format!("device {index}: {label}"));
            }
        }
    }

    fn devices_on(&self, account: usize) -> Vec<usize> {
        self.live_devices()
            .into_iter()
            .filter(|index| self.devices[*index].as_ref().unwrap().account == Some(account))
            .collect()
    }

    fn converged(&self, indexes: &[usize]) -> bool {
        let Some((first, rest)) = indexes.split_first() else {
            return true;
        };
        let reference = self.view(*first).convergence_lines();
        rest.iter()
            .all(|index| self.view(*index).convergence_lines() == reference)
    }

    /// Sync every signed-in device until its account converges, then check the
    /// invariants a correct sync must hold.
    fn settle_and_assert(&mut self) {
        let before_settle: Vec<(usize, View)> = self
            .live_devices()
            .into_iter()
            .map(|index| (index, self.view(index)))
            .collect();

        for account in 0..self.accounts.len() {
            let indexes = self.devices_on(account);
            if indexes.is_empty() {
                continue;
            }
            for _ in 0..indexes.len() * 4 + 8 {
                for index in &indexes {
                    self.sync(*index, 0);
                }
                if self.converged(&indexes) {
                    // One more round so every pusher pulls its own echo.
                    for index in &indexes {
                        self.sync(*index, 0);
                    }
                    // Matching views are not enough: a device whose attempts
                    // kept failing (injected connection drops) can look
                    // converged while its edits are still queued. Keep going
                    // until every queue is empty, or the rounds run out and
                    // the leftover is reported as a wedge.
                    let queues_empty = indexes.iter().all(|index| {
                        self.devices[*index].as_mut().unwrap().pending_edit_count() == 0
                    });
                    if queues_empty && self.converged(&indexes) {
                        break;
                    }
                }
            }
        }

        let seed = self.seed;
        for (index, before) in &before_settle {
            let after = self.view(*index);
            for violation in self
                .attribution
                .check_passive(*index, "settle", before, &after, false)
            {
                self.violations.push(format!("settle: {violation}"));
            }
        }

        let mut failures = std::mem::take(&mut self.violations);
        for account in 0..self.accounts.len() {
            let indexes = self.devices_on(account);
            let Some(&first) = indexes.first() else {
                continue;
            };
            for index in &indexes {
                let pending = self.devices[*index].as_mut().unwrap().pending_edit_count();
                if pending > 0 {
                    failures.push(format!(
                        "account {account}: device {index} still has {pending} unpushed edit(s) after settling (wedged)"
                    ));
                }
            }
            let reference = self.view(first).convergence_lines();
            for index in &indexes[1..] {
                let lines = self.view(*index).convergence_lines();
                if lines != reference {
                    let (only_first, only_other) = diff_lines(&reference, &lines);
                    failures.push(format!(
                        "account {account}: devices {first} and {index} diverged\n  only on {first}: {only_first:#?}\n  only on {index}: {only_other:#?}"
                    ));
                }
            }
            // A brand-new install signing in must see exactly the same content.
            let fresh = self.add_device(Some(account));
            for _ in 0..3 {
                self.sync(fresh, 0);
            }
            let fresh_lines = self.view(fresh).convergence_lines();
            let existing: Vec<String> = reference
                .iter()
                .filter(|line| !fresh_lines.contains(line))
                .cloned()
                .collect();
            if !existing.is_empty() {
                failures.push(format!(
                    "account {account}: a fresh device is missing content existing devices have (server lost it): {existing:#?}"
                ));
            }
        }
        failures.extend(std::mem::take(&mut self.violations));
        assert!(
            failures.is_empty(),
            "seed {seed}: {} sync invariant violation(s):\n{}",
            failures.len(),
            failures.join("\n")
        );
    }
}

impl Drop for World {
    fn drop(&mut self) {
        if std::env::var("KNOTQ_FUZZ_KEEP").is_err() && !std::thread::panicking() {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn run_seed(seed: u64, config: Config) {
    let steps = config.steps;
    let mut world = World::new(seed, config);
    for _ in 0..steps {
        world.step();
    }
    world.settle_and_assert();
}

fn run_seeds(first_seed: u64, config: impl Fn() -> Config + Sync) {
    // Belt and braces: nothing here resolves the global data directory, but a
    // stray call must never reach the user's real KnotQ data.
    static GUARD: std::sync::Once = std::sync::Once::new();
    GUARD.call_once(|| {
        std::env::set_var(
            "KNOTQ_DATA_DIR",
            std::env::temp_dir().join(format!(
                "knotq-production-fuzz-guard-{}",
                std::process::id()
            )),
        );
    });
    let seeds = env_usize("KNOTQ_FUZZ_SEEDS", 6);
    let workers = env_usize(
        "KNOTQ_FUZZ_WORKERS",
        std::thread::available_parallelism().map_or(2, |n| n.get().min(4)),
    )
    .clamp(1, seeds.max(1));
    let next = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| loop {
                    let offset = next.fetch_add(1, Ordering::Relaxed);
                    if offset >= seeds {
                        break;
                    }
                    run_seed(first_seed + offset as u64, config());
                })
            })
            .collect();
        for handle in handles {
            handle.join().expect("production fuzz worker panicked");
        }
    });
}

/// Several accounts, devices joining, switching accounts, crashing, relaunching,
/// and a backend that drops connections and loses acknowledgements.
#[test]
fn desktop_production_sync_fuzz() {
    run_seeds(1, || Config {
        accounts: 2,
        initial_devices: 3,
        max_devices: 5,
        steps: env_usize("KNOTQ_FUZZ_STEPS", 120),
        chaos: true,
    });
}

/// One account, no faults: the everyday multi-device case, where any loss is
/// unambiguous.
#[test]
fn desktop_production_single_account_fuzz() {
    run_seeds(10_000, || Config {
        accounts: 1,
        initial_devices: 3,
        max_devices: 4,
        steps: env_usize("KNOTQ_FUZZ_STEPS", 120),
        chaos: false,
    });
}

#[test]
#[ignore = "triage helper; replays KNOTQ_REPRO_SEED with the chaos configuration"]
fn replay_production_seed() {
    let seed = env_usize("KNOTQ_REPRO_SEED", 1) as u64;
    let chaos = std::env::var("KNOTQ_REPRO_PLAIN").is_err();
    run_seed(
        seed,
        Config {
            accounts: if chaos { 2 } else { 1 },
            initial_devices: 3,
            max_devices: if chaos { 5 } else { 4 },
            steps: env_usize("KNOTQ_FUZZ_STEPS", 120),
            chaos,
        },
    );
}
