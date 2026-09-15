//! Scripted production-path scenarios: each is a specific shape the random
//! fuzzer found or that a user hits every day, run through the same real
//! devices and the same invariant checks (attribution, server audit,
//! convergence, a fresh device joining).

use chrono::{Duration, NaiveDate};
use knotq_commands::Command;
use knotq_model::{Item, SchemeId};
use knotq_storage_json::load_daily_queue_scheme;

use super::device::{CrashPoint, DesktopDevice};
use super::{Config, World};

fn world(seed: u64, accounts: usize) -> World {
    World::new(
        seed,
        Config {
            accounts,
            initial_devices: 0,
            max_devices: 12,
            steps: 0,
            chaos: false,
        },
    )
}

fn open_day(device: &mut DesktopDevice, date: NaiveDate) -> SchemeId {
    let path = device.workspace_path.clone();
    device
        .state
        .ensure_daily_queue(date, || load_daily_queue_scheme(&path, date))
        .unwrap_or_else(|(id, reason)| panic!("open {date}: {reason} ({id})"))
        .0
}

fn add_line(device: &mut DesktopDevice, scheme: SchemeId, text: &str) {
    let position = device
        .state
        .workspace
        .scheme(scheme)
        .map_or(0, |scheme| scheme.items.len());
    device
        .state
        .apply_command(Command::InsertItem {
            scheme,
            position,
            item: Item::new(text),
        })
        .expect("insert line");
}

fn create_scheme(device: &mut DesktopDevice, name: &str) {
    let root = device.state.workspace.root;
    device
        .state
        .apply_command(Command::CreateScheme {
            folder: root,
            name: name.to_string(),
            color_index: 1,
            position: None,
        })
        .expect("create scheme");
}

/// Device A fills the account: a scheme, a past day with a line.
fn seed_account(world: &mut World) -> usize {
    let a = world.add_device(Some(0));
    let past = World::today() - Duration::days(8);
    world.local(a, |device, _| {
        create_scheme(device, "Existing plans");
        let day = open_day(device, past);
        add_line(device, day, "on a past day");
    });
    world.sync(a, 0);
    a
}

/// A second install's offline use before its first sync.
fn use_offline(world: &mut World, device: usize) {
    world.local(device, |device, _| {
        let today = device.today();
        let day = open_day(device, today);
        add_line(device, day, "offline today");
        create_scheme(device, "Made offline");
    });
}

#[test]
fn new_install_with_offline_edits_joins_the_account() {
    let mut world = world(90_001, 1);
    seed_account(&mut world);
    let b = world.add_device(Some(0));
    use_offline(&mut world, b);
    world.sync(b, 0);
    world.settle_and_assert();
}

/// Fuzz seed 4: a fresh install signs in, and while its first sync is in flight
/// the user moves a starter line to another scheme. The landed result must hold
/// that line exactly once — it came back in both schemes.
#[test]
fn a_line_moved_while_the_first_sync_is_in_flight_lands_once() {
    let mut world = world(90_010, 1);
    let a = world.add_device(Some(0));
    world.sync(a, 0);
    let b = world.add_device(Some(0));
    let run = {
        let device = world.devices[b].as_mut().unwrap();
        device.run_sync(&world.accounts[0], false)
    }
    .expect("signed in");
    world.local(b, |device, _| {
        let workspace = device.state.workspace.clone();
        let mut schemes = workspace
            .schemes
            .values()
            .filter(|scheme| !scheme.items.is_empty())
            .map(|scheme| scheme.id);
        let source = schemes.next().expect("starter scheme with lines");
        let target = schemes.next().expect("second starter scheme");
        let moved = workspace.scheme(source).unwrap().items[0].clone();
        let position = workspace.scheme(target).unwrap().items.len();
        device
            .state
            .apply_command(Command::Batch(vec![
                Command::DeleteItem {
                    scheme: source,
                    item: moved.id,
                },
                Command::InsertItem {
                    scheme: target,
                    position,
                    item: moved,
                },
            ]))
            .expect("move line");
    });
    let error = world.devices[b].as_mut().unwrap().land_sync(run);
    assert!(error.is_none(), "first sync failed: {error:?}");
    world.settle_and_assert();
}

/// A fresh install deletes one starter line and retypes another before it ever
/// syncs, then joins an account that already holds the same starter lines. The
/// delete and the edit must carry over: the deleted line must not come back and
/// no line may end up with its text repeated.
#[test]
fn starter_lines_edited_before_the_first_sync_join_the_account_once() {
    let mut world = world(90_011, 1);
    let a = world.add_device(Some(0));
    world.sync(a, 0);
    let b = world.add_device(Some(0));
    let (source, deleted, edited) = world.local(b, |device, _| {
        let workspace = device.state.workspace.clone();
        let scheme = workspace
            .schemes
            .values()
            .filter(|scheme| scheme.items.len() >= 2)
            .min_by_key(|scheme| scheme.id)
            .expect("starter scheme with two lines")
            .clone();
        let deleted = scheme.items[0].id;
        let edited = scheme.items[1].id;
        device
            .state
            .apply_command(Command::Batch(vec![
                Command::DeleteItem {
                    scheme: scheme.id,
                    item: deleted,
                },
                Command::UpdateItemText {
                    scheme: scheme.id,
                    item: edited,
                    text: "retyped offline".to_string(),
                },
            ]))
            .expect("offline starter edits");
        (scheme.id, deleted, edited)
    });
    world.sync(b, 0);
    world.settle_and_assert();
    for index in [a, b] {
        let device = world.devices[index].as_ref().unwrap();
        let scheme = device
            .state
            .workspace
            .scheme(source)
            .expect("starter scheme");
        assert!(
            scheme.item(deleted).is_none(),
            "device {index}: the line deleted offline came back"
        );
        assert_eq!(
            scheme.item(edited).map(|item| item.text()),
            Some("retyped offline".to_string()),
            "device {index}: the line retyped offline did not land once"
        );
    }
}

/// Fuzz seed 3: the app dies after the save task wrote the pending queue but
/// before it wrote the CRDT state. The relaunched session must not author its
/// next edit beside the queued one it never saw — pushing the queue would then
/// let the stale edit win and revert what the user typed after the relaunch.
/// Client ids are random, so each round is a fresh coin flip on `HEAD`.
#[test]
fn an_edit_after_a_crash_between_the_queue_and_crdt_saves_is_kept() {
    let mut world = world(90_012, 1);
    let a = world.add_device(Some(0));
    world.sync(a, 0);
    for round in 0..6 {
        let (scheme, item) = world.local(a, |device, _| {
            let today = device.today();
            let day = open_day(device, today);
            add_line(device, day, "typed");
            let item = device
                .state
                .workspace
                .scheme(day)
                .unwrap()
                .items
                .last()
                .unwrap()
                .id;
            (day, item)
        });
        world.sync(a, 0);
        let retype = |world: &mut World, text: String| {
            world.local(a, |device, _| {
                device
                    .state
                    .apply_command(Command::UpdateItemText { scheme, item, text })
                    .expect("retype line");
            });
        };
        retype(&mut world, format!("queued {round}"));
        let device = world.devices[a].take().unwrap();
        world.devices[a] = Some(device.crash(CrashPoint::AfterPending));
        retype(&mut world, format!("after relaunch {round}"));
        world.sync(a, 0);
        let text = world.devices[a]
            .as_ref()
            .unwrap()
            .state
            .workspace
            .scheme(scheme)
            .and_then(|scheme| scheme.item(item))
            .map(|item| item.text());
        assert_eq!(
            text,
            Some(format!("after relaunch {round}")),
            "round {round}"
        );
    }
    world.settle_and_assert();
}

/// KNOWN GAP — kept runnable so the fix can be proven against it.
///
/// Fuzz seed 5: a fresh install's first sync lands through the merge while an
/// edit made during the run is still unpushed. That edit's index update names
/// the pre-sign-in root, which must be folded into the account's root — not
/// left behind as an empty second "root" folder in the sidebar.
///
/// Calling `reroot_pre_sign_in_edits` at the end of `merge_sync_crdt_states`
/// (as `replace_from_sync` does) was tried twice and is far worse: its index
/// repair loses folders and Daily lines across the production fuzz (9 -> 42
/// violations on 2026-09-15). The re-root must happen inside the CRDT index
/// without rewriting entries the store's plain workspace does not hold.
#[test]
#[ignore = "known gap: first-sync merge leaves the pre-sign-in root as a second root folder"]
fn a_first_sync_with_an_in_flight_edit_leaves_no_second_root_folder() {
    let mut world = world(90_013, 1);
    let a = world.add_device(Some(0));
    world.sync(a, 0);
    let b = world.add_device(Some(0));
    let run = {
        let device = world.devices[b].as_mut().unwrap();
        device.run_sync(&world.accounts[0], false)
    }
    .expect("signed in");
    world.local(b, |device, _| {
        let today = device.today();
        let day = open_day(device, today);
        add_line(device, day, "typed during the first sync");
        create_scheme(device, "Made during the first sync");
    });
    let error = world.devices[b].as_mut().unwrap().land_sync(run);
    assert!(error.is_none(), "first sync failed: {error:?}");
    let stray_roots = |device: &DesktopDevice| -> Vec<String> {
        let workspace = &device.state.workspace;
        workspace
            .folders
            .values()
            .filter(|folder| folder.id != workspace.root && folder.name == "root")
            .map(|folder| folder.id.to_string())
            .collect()
    };
    let landed = stray_roots(world.devices[b].as_ref().unwrap());
    assert!(
        landed.is_empty(),
        "landing left a second root folder: {landed:?}"
    );
    world.relaunch(b);
    let relaunched = stray_roots(world.devices[b].as_ref().unwrap());
    assert!(
        relaunched.is_empty(),
        "relaunch kept a second root folder: {relaunched:?}"
    );
    world.settle_and_assert();
}

#[test]
fn new_install_relaunched_before_its_first_sync_joins_the_account() {
    let mut world = world(90_002, 1);
    seed_account(&mut world);
    let b = world.add_device(Some(0));
    use_offline(&mut world, b);
    world.relaunch(b);
    world.sync(b, 0);
    world.settle_and_assert();
}

#[test]
fn new_install_saved_before_its_first_sync_joins_the_account() {
    let mut world = world(90_003, 1);
    seed_account(&mut world);
    let b = world.add_device(Some(0));
    use_offline(&mut world, b);
    let _ = world.devices[b].as_mut().unwrap().save();
    world.sync(b, 0);
    world.settle_and_assert();
}

#[test]
fn install_switched_to_another_account_before_its_first_sync_joins_it() {
    let mut world = world(90_004, 2);
    seed_account(&mut world);
    let b = world.add_device(Some(1));
    use_offline(&mut world, b);
    let device = world.devices[b].as_mut().unwrap();
    device.sign_in(&world.accounts[0]);
    world.sync(b, 0);
    world.settle_and_assert();
}

/// KNOWN GAP — kept runnable so the fix can be proven against it.
///
/// A brand-new install that has never synced crashes after the save task wrote
/// its workspace files but before it wrote the pending queue and CRDT state.
/// On relaunch its edits exist only in the plain files: the CRDT is unseeded
/// and nothing is queued, so the first sync adopts the account's index and
/// those edits disappear. `HEAD` behaves the same way.
///
/// Seeding the CRDT from the plain files at launch was tried and made things
/// strictly worse: it pushes an entire index lineage authored under the
/// pre-sign-in identity, which loses content elsewhere. The real fix is to
/// re-root a pre-sign-in lineage inside the CRDT index at first sign-in.
#[test]
#[ignore = "known gap: never-synced install crashing between its workspace and CRDT saves"]
fn new_install_crashed_before_its_first_sync_joins_the_account() {
    let mut world = world(90_005, 1);
    seed_account(&mut world);
    let b = world.add_device(Some(0));
    use_offline(&mut world, b);
    let device = world.devices[b].take().unwrap();
    world.devices[b] = Some(device.crash(CrashPoint::AfterWorkspace));
    world.sync(b, 0);
    world.settle_and_assert();
}

// --- bisecting which part of a real first launch breaks the join -------------

/// Install device `index` from a chosen pre-launch workspace, signed into
/// account 0, attributing its content to it as the fuzzer does for a seed.
fn install_prepared(world: &mut World, workspace: knotq_model::Workspace) -> usize {
    let index = world.devices.len();
    let dir = world.root.join(format!("device-{index}"));
    let mut device = DesktopDevice::install_with(index, dir, World::today(), &workspace);
    let seeded = super::oracle::View::of(&device.full_workspace());
    world.attribution.record_seed(index, &seeded);
    device.sign_in(&world.accounts[0]);
    world.devices.push(Some(device));
    index
}

#[test]
fn join_variant_starter_without_offline_edits() {
    let mut world = world(90_101, 1);
    seed_account(&mut world);
    let b = world.add_device(Some(0));
    world.sync(b, 0);
    world.settle_and_assert();
}

#[test]
fn join_variant_empty_workspace_with_offline_edits() {
    let mut world = world(90_102, 1);
    seed_account(&mut world);
    let b = install_prepared(&mut world, knotq_model::Workspace::new());
    use_offline(&mut world, b);
    world.sync(b, 0);
    world.settle_and_assert();
}

#[test]
fn join_variant_starter_already_on_the_account_identity_with_offline_edits() {
    let mut world = world(90_103, 1);
    seed_account(&mut world);
    let mut starter = knotq_state::make_default_workspace_for_date(World::today());
    starter.canonicalize_personal_sync_identity(world.accounts[0].workspace);
    let b = install_prepared(&mut world, starter);
    use_offline(&mut world, b);
    world.sync(b, 0);
    world.settle_and_assert();
}

#[test]
fn join_variant_empty_workspace_on_the_account_identity_with_offline_edits() {
    let mut world = world(90_104, 1);
    seed_account(&mut world);
    let mut empty = knotq_model::Workspace::new();
    empty.canonicalize_personal_sync_identity(world.accounts[0].workspace);
    let b = install_prepared(&mut world, empty);
    use_offline(&mut world, b);
    world.sync(b, 0);
    world.settle_and_assert();
}
