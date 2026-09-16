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

/// KNOWN GAP — kept runnable so the fix can be proven against it.
///
/// Fuzz seed 1: a fresh install recolours a scheme while its first sync is in
/// flight, and the colour never reaches the account.
///
/// Part of it is fixed: landing used to clear the in-flight edit as pushed
/// because the run pushed through the same sequence under an edit of its own
/// (`in_flight_landing_tests`). What remains is the workspace index. A
/// never-synced install has no saved index state, so its first index write is a
/// full population authored under a random client id — with the new colour
/// already in it. The account's index is an equally full population from
/// another device, so every scheme entry is a concurrent pair and the account's
/// entry can win on client id alone. The fix mirrors scheme content: populate
/// the index from the pre-edit workspace under a deterministic client id, then
/// write the edit as a delta after it.
#[test]
#[ignore = "known gap: a never-synced install's index population loses to the account's on first sign-in"]
fn an_edit_made_while_a_sync_is_in_flight_is_pushed() {
    let mut world = world(90_014, 1);
    let a = world.add_device(Some(0));
    world.sync(a, 0);
    let b = world.add_device(Some(0));
    let run = {
        let device = world.devices[b].as_mut().unwrap();
        device.run_sync(&world.accounts[0], false)
    }
    .expect("signed in");
    let recoloured = world.local(b, |device, _| {
        let workspace = device.state.workspace.clone();
        let scheme = workspace
            .schemes
            .values()
            .filter(|scheme| !workspace.daily_queue.values().any(|id| *id == scheme.id))
            .min_by_key(|scheme| scheme.id)
            .expect("a starter scheme")
            .id;
        device
            .state
            .apply_command(Command::SetSchemeColor {
                id: scheme,
                color_index: 7,
            })
            .expect("recolour scheme");
        scheme
    });
    let error = world.devices[b].as_mut().unwrap().land_sync(run);
    assert!(error.is_none(), "first sync failed: {error:?}");
    world.sync(b, 0);
    world.sync(a, 0);
    let colour = world.devices[a]
        .as_ref()
        .unwrap()
        .state
        .workspace
        .scheme(recoloured)
        .map(|scheme| scheme.color_index);
    assert_eq!(
        colour,
        Some(7),
        "the colour chosen during the sync never reached the other device"
    );
    world.settle_and_assert();
}

/// Fuzz seed 10005: a folder archived and synced before the save task runs.
/// The run works on the saved workspace overlaid with the in-memory one, and
/// the overlay did not carry the folder archive — so the run saw a folder that
/// was in neither the tree nor the trash, dropped it, and pushed an index
/// without it. The folder vanished for every device instead of moving to the
/// trash, and a scheme another device had moved into it escaped to the root.
#[test]
fn a_folder_archived_just_before_a_sync_stays_in_the_trash() {
    let mut world = world(90_015, 1);
    let a = world.add_device(Some(0));
    let folder = world.local(a, |device, _| {
        let root = device.state.workspace.root;
        device
            .state
            .apply_command(Command::CreateFolder {
                parent: root,
                name: "Archive me".to_string(),
                position: None,
            })
            .expect("create folder");
        device
            .state
            .workspace
            .folders
            .values()
            .find(|folder| folder.name == "Archive me")
            .expect("created folder")
            .id
    });
    world.sync(a, 0);
    let b = world.add_device(Some(0));
    world.sync(b, 0);
    world.local(a, |device, _| {
        device
            .state
            .apply_command(Command::DeleteFolder { id: folder })
            .expect("archive folder");
    });
    // No save between the archive and the sync.
    let run = {
        let device = world.devices[a].as_mut().unwrap();
        device.run_sync(&world.accounts[0], false)
    }
    .expect("signed in");
    let error = world.devices[a].as_mut().unwrap().land_sync(run);
    assert!(error.is_none(), "sync failed: {error:?}");
    world.sync(b, 0);
    for index in [a, b] {
        let workspace = &world.devices[index].as_ref().unwrap().state.workspace;
        assert!(
            workspace.folders.contains_key(&folder)
                && workspace.recently_deleted_folders.contains(&folder),
            "device {index}: the folder archived before the sync is not in the trash"
        );
    }
    world.settle_and_assert();
}

/// Fuzz seed 6: a sync run pulls and saves, then fails before landing (its push
/// response is lost). The store never received what the run pulled, but the
/// pull cursor moved past it. The next run must start from the saved state
/// merged with the store's, not the store's alone — or the pulled change is
/// dropped and the server never resends it.
#[test]
fn a_change_pulled_by_a_run_that_failed_before_landing_is_not_lost() {
    let mut world = world(90_016, 1);
    let a = world.add_device(Some(0));
    world.sync(a, 0);
    let b = world.add_device(Some(0));
    world.sync(b, 0);
    world.sync(a, 0);
    let (scheme, line) = world.local(a, |device, _| {
        let workspace = device.state.workspace.clone();
        let scheme = workspace
            .schemes
            .values()
            .filter(|scheme| {
                !workspace.daily_queue.values().any(|id| *id == scheme.id)
                    && scheme.items.len() >= 2
            })
            .min_by_key(|scheme| scheme.id)
            .expect("a starter scheme with lines")
            .clone();
        let line = scheme.items[0].id;
        device
            .state
            .apply_command(Command::DeleteItem {
                scheme: scheme.id,
                item: line,
            })
            .expect("delete line");
        (scheme.id, line)
    });
    world.sync(a, 0);
    // Device b has an unpushed edit elsewhere, so its run pushes; the server
    // applies that push but the response never arrives.
    world.local(b, |device, _| {
        let today = device.today();
        let day = open_day(device, today);
        add_line(device, day, "pushed, response lost");
    });
    world.accounts[0].server.lose_next_push_responses(1);
    let run = {
        let device = world.devices[b].as_mut().unwrap();
        device.run_sync(&world.accounts[0], false)
    }
    .expect("signed in");
    let error = world.devices[b].as_mut().unwrap().land_sync(run);
    assert!(
        error.is_some(),
        "the run should fail on its lost push response"
    );
    let run = {
        let device = world.devices[b].as_mut().unwrap();
        device.run_sync(&world.accounts[0], false)
    }
    .expect("signed in");
    let error = world.devices[b].as_mut().unwrap().land_sync(run);
    assert!(error.is_none(), "the retry failed: {error:?}");
    let line_came_back = world.devices[b]
        .as_ref()
        .unwrap()
        .state
        .workspace
        .scheme(scheme)
        .is_some_and(|scheme| scheme.item(line).is_some());
    assert!(
        !line_came_back,
        "a line deleted on another device is back after a run that failed before landing"
    );
    world.settle_and_assert();
}

/// Deep production fuzz, seed 1: a sync run saves the pulled workspace (here,
/// another device's move of a line), and before it lands the save task writes
/// the store's older workspace over those files; then the app dies. On relaunch
/// the plain files still show the line in its source scheme while the CRDT has
/// it deleted there, and the next edit to that scheme re-expresses the stale
/// copy — the moved line comes back in its source scheme for every device.
#[test]
fn a_save_while_a_sync_is_in_flight_cannot_bring_a_moved_line_back() {
    let mut world = world(90_017, 1);
    let a = world.add_device(Some(0));
    world.sync(a, 0);
    let b = world.add_device(Some(0));
    world.sync(b, 0);
    world.sync(a, 0);
    let (source, target, line) = world.local(a, |device, _| {
        let workspace = device.state.workspace.clone();
        let mut schemes: Vec<_> = workspace
            .schemes
            .values()
            .filter(|scheme| {
                !workspace.daily_queue.values().any(|id| *id == scheme.id)
                    && scheme.items.len() >= 2
            })
            .map(|scheme| scheme.id)
            .collect();
        schemes.sort();
        let (source, target) = (schemes[0], schemes[1]);
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
                    item: moved.clone(),
                },
            ]))
            .expect("move line");
        (source, target, moved.id)
    });
    world.sync(a, 0);

    // Device b's run pulls the move and saves it; the save task runs before the
    // run lands, and the app dies before the landing.
    let run = {
        let device = world.devices[b].as_mut().unwrap();
        device.run_sync(&world.accounts[0], false)
    }
    .expect("signed in");
    let _ = world.devices[b].as_mut().unwrap().save();
    drop(run);
    let device = world.devices[b].take().unwrap();
    world.devices[b] = Some(device.crash(CrashPoint::AfterWorkspace));

    // After relaunch, b edits the source scheme and syncs.
    world.local(b, |device, _| {
        let other = device
            .state
            .workspace
            .scheme(source)
            .unwrap()
            .items
            .iter()
            .find(|item| item.id != line)
            .expect("another line")
            .id;
        device
            .state
            .apply_command(Command::UpdateItemText {
                scheme: source,
                item: other,
                text: "edited after the relaunch".to_string(),
            })
            .expect("edit the source scheme");
    });
    world.sync(b, 0);
    world.sync(a, 0);
    for index in [a, b] {
        let workspace = &world.devices[index].as_ref().unwrap().state.workspace;
        let holders: Vec<_> = workspace
            .schemes
            .values()
            .filter(|scheme| scheme.item(line).is_some())
            .map(|scheme| scheme.id)
            .collect();
        assert_eq!(
            holders,
            vec![target],
            "device {index}: the moved line should be only in its target scheme"
        );
    }
    world.settle_and_assert();
}

/// Deep production fuzz, seeds 10000 and 10005: a line is retyped on one device
/// while another moves it to a different scheme (carry-over does exactly this).
/// Each scheme is its own document, so the move deletes the line from its source
/// and inserts a copy — carrying the text the moving device saw — into the
/// target. The retype landed on the source copy and was lost for every device.
#[test]
fn a_line_retyped_while_another_device_moves_it_keeps_the_new_text() {
    let mut world = world(90_018, 1);
    let a = world.add_device(Some(0));
    world.sync(a, 0);
    let b = world.add_device(Some(0));
    world.sync(b, 0);
    world.sync(a, 0);
    let (source, target, line) = {
        let workspace = world.devices[a].as_ref().unwrap().state.workspace.clone();
        let mut schemes: Vec<_> = workspace
            .schemes
            .values()
            .filter(|scheme| {
                !workspace.daily_queue.values().any(|id| *id == scheme.id)
                    && scheme.items.len() >= 2
            })
            .map(|scheme| scheme.id)
            .collect();
        schemes.sort();
        let source = schemes[0];
        (
            source,
            schemes[1],
            workspace.scheme(source).unwrap().items[0].id,
        )
    };
    // Device b retypes the line and does not sync yet.
    world.local(b, |device, _| {
        device
            .state
            .apply_command(Command::UpdateItemText {
                scheme: source,
                item: line,
                text: "retyped on b".to_string(),
            })
            .expect("retype line");
    });
    // Device a moves the same line to another scheme and syncs.
    world.local(a, |device, _| {
        let workspace = device.state.workspace.clone();
        let moved = workspace
            .scheme(source)
            .unwrap()
            .item(line)
            .unwrap()
            .clone();
        let position = workspace.scheme(target).unwrap().items.len();
        device
            .state
            .apply_command(Command::Batch(vec![
                Command::DeleteItem {
                    scheme: source,
                    item: line,
                },
                Command::InsertItem {
                    scheme: target,
                    position,
                    item: moved,
                },
            ]))
            .expect("move line");
    });
    world.sync(a, 0);
    // Device b pulls the move; its own retype has to survive the landing.
    world.sync(b, 0);
    // b pushes what it re-applied, and a pulls it.
    world.sync(b, 0);
    world.sync(a, 0);
    for index in [b, a] {
        let workspace = &world.devices[index].as_ref().unwrap().state.workspace;
        let holders: Vec<_> = workspace
            .schemes
            .values()
            .filter_map(|scheme| scheme.item(line).map(|item| (scheme.id, item.text())))
            .collect();
        assert_eq!(
            holders,
            vec![(target, "retyped on b".to_string())],
            "device {index}: the moved line should be in its target scheme with b's text"
        );
    }
    world.settle_and_assert();
}

/// A sync run saves another device's move of a line, and the user quits before
/// the run lands. The shutdown flush writes the store's older workspace over the
/// run's files, and with the run's cursors kept the move was never pulled again:
/// the next edit to the source scheme brought the moved line back for every
/// device. The quit now resets the cursors of the documents the run pulled.
#[test]
fn quitting_while_a_sync_is_in_flight_cannot_bring_a_moved_line_back() {
    let mut world = world(90_019, 1);
    let a = world.add_device(Some(0));
    world.sync(a, 0);
    let b = world.add_device(Some(0));
    world.sync(b, 0);
    world.sync(a, 0);
    let (source, target, line) = world.local(a, |device, _| {
        let workspace = device.state.workspace.clone();
        let mut schemes: Vec<_> = workspace
            .schemes
            .values()
            .filter(|scheme| {
                !workspace.daily_queue.values().any(|id| *id == scheme.id)
                    && scheme.items.len() >= 2
            })
            .map(|scheme| scheme.id)
            .collect();
        schemes.sort();
        let (source, target) = (schemes[0], schemes[1]);
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
                    item: moved.clone(),
                },
            ]))
            .expect("move line");
        (source, target, moved.id)
    });
    world.sync(a, 0);

    // Device b's run pulls the move and saves it; the user quits before it lands.
    let run = {
        let device = world.devices[b].as_mut().unwrap();
        device.run_sync(&world.accounts[0], false)
    }
    .expect("signed in");
    drop(run);
    world.relaunch(b);

    world.local(b, |device, _| {
        let other = device
            .state
            .workspace
            .scheme(source)
            .unwrap()
            .items
            .iter()
            .find(|item| item.id != line)
            .expect("another line")
            .id;
        device
            .state
            .apply_command(Command::UpdateItemText {
                scheme: source,
                item: other,
                text: "edited after the relaunch".to_string(),
            })
            .expect("edit the source scheme");
    });
    world.sync(b, 0);
    world.sync(a, 0);
    for index in [a, b] {
        let workspace = &world.devices[index].as_ref().unwrap().state.workspace;
        let holders: Vec<_> = workspace
            .schemes
            .values()
            .filter(|scheme| scheme.item(line).is_some())
            .map(|scheme| scheme.id)
            .collect();
        assert_eq!(
            holders,
            vec![target],
            "device {index}: the moved line should be only in its target scheme"
        );
    }
    world.settle_and_assert();
}

/// Deep production fuzz, seed 10000 step 179: a line is retyped, the app is
/// saved and relaunched before it syncs, and meanwhile another device moves the
/// line to a different scheme. The relaunch emptied the store's queued
/// operations, so landing no longer knew the line's text had been edited and the
/// moved copy's older text won.
#[test]
fn a_line_retyped_before_a_relaunch_keeps_its_text_when_another_device_moves_it() {
    let mut world = world(90_020, 1);
    let a = world.add_device(Some(0));
    world.sync(a, 0);
    let b = world.add_device(Some(0));
    world.sync(b, 0);
    world.sync(a, 0);
    let (source, target, line) = {
        let workspace = world.devices[a].as_ref().unwrap().state.workspace.clone();
        let mut schemes: Vec<_> = workspace
            .schemes
            .values()
            .filter(|scheme| {
                !workspace.daily_queue.values().any(|id| *id == scheme.id)
                    && scheme.items.len() >= 2
            })
            .map(|scheme| scheme.id)
            .collect();
        schemes.sort();
        let source = schemes[0];
        (
            source,
            schemes[1],
            workspace.scheme(source).unwrap().items[0].id,
        )
    };
    world.local(b, |device, _| {
        device
            .state
            .apply_command(Command::UpdateItemText {
                scheme: source,
                item: line,
                text: "retyped before the relaunch".to_string(),
            })
            .expect("retype line");
    });
    // One more command after the retype: a deferred CRDT flush attaches its
    // updates to the NEWEST queued operation, so the retype's bytes ride on this
    // later one. A record describing only its own operation's command loses the
    // retype entirely.
    world.local(b, |device, _| {
        let indented = device
            .state
            .workspace
            .scheme(source)
            .unwrap()
            .items
            .last()
            .unwrap()
            .id;
        device
            .state
            .apply_command(Command::SetItemIndent {
                scheme: source,
                item: indented,
                indent: 1,
            })
            .expect("indent another line");
    });
    let _ = world.devices[b].as_mut().unwrap().save();
    world.relaunch(b);
    world.local(a, |device, _| {
        let workspace = device.state.workspace.clone();
        let moved = workspace
            .scheme(source)
            .unwrap()
            .item(line)
            .unwrap()
            .clone();
        let position = workspace.scheme(target).unwrap().items.len();
        device
            .state
            .apply_command(Command::Batch(vec![
                Command::DeleteItem {
                    scheme: source,
                    item: line,
                },
                Command::InsertItem {
                    scheme: target,
                    position,
                    item: moved,
                },
            ]))
            .expect("move line");
    });
    world.sync(a, 0);
    world.sync(b, 0);
    world.sync(b, 0);
    world.sync(a, 0);
    for index in [b, a] {
        let workspace = &world.devices[index].as_ref().unwrap().state.workspace;
        let holders: Vec<_> = workspace
            .schemes
            .values()
            .filter_map(|scheme| scheme.item(line).map(|item| (scheme.id, item.text())))
            .collect();
        assert_eq!(
            holders,
            vec![(target, "retyped before the relaunch".to_string())],
            "device {index}: the moved line should be in its target scheme with b's text"
        );
    }
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
/// Two devices change DIFFERENT fields of the same scheme, neither having seen
/// the other's change.
///
/// The workspace index keeps every field of a node — name, colour, gsync,
/// source, and its membership parent and position — inside ONE map value, so
/// the merge resolves the whole node by client id and silently discards one
/// device's edit. `write_item_fields` fixed exactly this for item metadata
/// ("writing every field on any edit let a device that changed one attribute
/// silently restore its stale copy of every other attribute"); the index never
/// got the same treatment.
///
/// The scheme is CREATED by one device rather than taken from the starter
/// workspace on purpose: `Attribution::record_seed` marks every starter field as
/// written by every device, which would explain away the very revert this pins.
#[test]
fn two_devices_changing_different_fields_of_one_scheme_keep_both() {
    let mut world = world(90_120, 1);
    let a = world.add_device(Some(0));
    world.local(a, |device, _| create_scheme(device, "Shared plans"));
    world.sync(a, 0);
    let b = world.add_device(Some(0));
    world.sync(b, 0);
    world.sync(a, 0);

    let scheme = {
        let workspace = world.devices[a].as_ref().unwrap().state.workspace.clone();
        workspace
            .schemes
            .values()
            .find(|scheme| scheme.name == "Shared plans")
            .expect("the created scheme")
            .id
    };

    // Concurrent by construction: neither device syncs between these two edits,
    // so neither has seen the other's when both are pushed.
    world.local(a, |device, _| {
        device
            .state
            .apply_command(Command::RenameScheme {
                id: scheme,
                name: "renamed on a".to_string(),
            })
            .expect("rename scheme");
    });
    world.local(b, |device, _| {
        device
            .state
            .apply_command(Command::SetSchemeColor {
                id: scheme,
                color_index: 7,
            })
            .expect("recolour scheme");
    });

    world.sync(a, 0);
    world.sync(b, 0);
    world.sync(a, 0);
    world.settle_and_assert();

    let workspace = world.devices[a].as_ref().unwrap().state.workspace.clone();
    let landed = workspace.scheme(scheme).expect("the scheme survives");
    assert_eq!(landed.name, "renamed on a", "the rename was discarded");
    assert_eq!(landed.color_index, 7, "the recolour was discarded");
}

/// A rename and a move of the same scheme, on two devices, both survive.
///
/// Companion to the test above: that one pairs two PAYLOAD fields (name,
/// colour), while this pairs a payload field with the MEMBERSHIP parent, which
/// still lives inside the whole-node `nodes` value rather than in `node_fields`.
///
/// HONEST SCOPE — this passes at HEAD and always has; it is a regression pin,
/// not a reproduction. Two things keep it from being the concurrency test its
/// name suggests, and both are worth knowing before trusting it:
///
///  1. `World::sync` PULLS BEFORE IT PUSHES, so in `sync(a); sync(b); sync(a)`
///     device b has already seen a's rename before it pushes its move. The two
///     edits are causally ordered, never concurrent at the CRDT level. Genuine
///     concurrency needs two replicas exchanging updates from a common base, as
///     `crdt::tests::workspace_materialization`'s
///     `concurrent_folder_additions_on_two_replicas_merge_without_loss` does.
///  2. The whole-node last-writer-wins class this was written to chase is
///     already fixed for payload fields by the `node_fields` split (e2f66c2).
///
/// Kept because the shape (rename + move of one scheme across two devices) is a
/// real user action with no other coverage, and a future change to membership
/// storage should not break it silently.
#[test]
fn two_devices_moving_and_renaming_one_scheme_keep_both() {
    let mut world = world(90_122, 1);
    let a = world.add_device(Some(0));
    let folder = world.local(a, |device, _| {
        let root = device.state.workspace.root;
        device
            .state
            .apply_command(Command::CreateFolder {
                parent: root,
                name: "Destination".to_string(),
                position: None,
            })
            .expect("create folder");
        device
            .state
            .workspace
            .folders
            .values()
            .find(|folder| folder.name == "Destination")
            .expect("the created folder")
            .id
    });
    world.local(a, |device, _| create_scheme(device, "Shared plans"));
    world.sync(a, 0);
    let b = world.add_device(Some(0));
    world.sync(b, 0);
    world.sync(a, 0);

    let scheme = {
        let workspace = world.devices[a].as_ref().unwrap().state.workspace.clone();
        workspace
            .schemes
            .values()
            .find(|scheme| scheme.name == "Shared plans")
            .expect("the created scheme")
            .id
    };

    // Concurrent by construction: neither device syncs between these two edits.
    world.local(a, |device, _| {
        device
            .state
            .apply_command(Command::RenameScheme {
                id: scheme,
                name: "renamed on a".to_string(),
            })
            .expect("rename scheme");
    });
    world.local(b, |device, _| {
        device
            .state
            .apply_command(Command::MoveNode {
                node: knotq_model::NodeRef::Scheme(scheme),
                new_parent: folder,
                position: 0,
            })
            .expect("move scheme into the folder");
    });

    world.sync(a, 0);
    world.sync(b, 0);
    world.sync(a, 0);
    world.settle_and_assert();

    let workspace = world.devices[a].as_ref().unwrap().state.workspace.clone();
    let landed = workspace.scheme(scheme).expect("the scheme survives");
    assert_eq!(landed.name, "renamed on a", "the rename was discarded");
    let parent = workspace
        .folders
        .values()
        .find(|candidate| {
            candidate
                .children
                .contains(&knotq_model::NodeRef::Scheme(scheme))
        })
        .map(|candidate| candidate.id);
    assert_eq!(
        parent,
        Some(folder),
        "the move was discarded and the scheme re-homed (root is {:?})",
        workspace.root
    );
}

/// Two devices each move one of two folders into the other, neither having seen
/// the other's move.
///
/// `move_node` rejects a cycle it can SEE (`CommandError::CycleMove`, checked
/// before any mutation), so neither command is invalid when it is issued — the
/// cycle exists only in the merged result. `normalize_folder_tree` then walks
/// only from the root, never visits either folder, and
/// `normalize_one_level_folders` drops both plus every scheme inside them; the
/// next index write publishes that as an authoritative deletion and every other
/// device faithfully pulls the loss. Production fuzz seed 10042 (device 0 moves
/// b7e06805 -> 627f8085 at step 179, device 2 moves it back at 184, and device
/// 2's sync at 198 takes 3 schemes and 3 folders off the server).
#[test]
fn two_devices_moving_folders_into_each_other_keep_both() {
    let mut world = world(90_121, 1);
    let a = world.add_device(Some(0));
    world.local(a, |device, _| {
        let root = device.state.workspace.root;
        for name in ["Outer", "Inner"] {
            device
                .state
                .apply_command(Command::CreateFolder {
                    parent: root,
                    name: name.to_string(),
                    position: None,
                })
                .expect("create folder");
        }
    });
    let folder_named = |world: &World, name: &str| {
        world.devices[a]
            .as_ref()
            .unwrap()
            .state
            .workspace
            .folders
            .values()
            .find(|folder| folder.name == name)
            .unwrap_or_else(|| panic!("folder {name} exists"))
            .id
    };
    let outer = folder_named(&world, "Outer");
    let inner = folder_named(&world, "Inner");
    // A scheme inside one of them, so what the cycle costs is content and not
    // just structure.
    world.local(a, |device, _| {
        device
            .state
            .apply_command(Command::CreateScheme {
                folder: inner,
                name: "Inside".to_string(),
                color_index: 3,
                position: None,
            })
            .expect("create scheme");
    });
    world.sync(a, 0);
    let b = world.add_device(Some(0));
    world.sync(b, 0);
    world.sync(a, 0);

    // Concurrent by construction: neither device syncs between these two moves,
    // so each one is legal against the view it is applied to.
    world.local(a, |device, _| {
        device
            .state
            .apply_command(Command::MoveNode {
                node: knotq_model::NodeRef::Folder(outer),
                new_parent: inner,
                position: 0,
            })
            .expect("move outer into inner");
    });
    world.local(b, |device, _| {
        device
            .state
            .apply_command(Command::MoveNode {
                node: knotq_model::NodeRef::Folder(inner),
                new_parent: outer,
                position: 0,
            })
            .expect("move inner into outer");
    });

    world.sync(a, 0);
    world.sync(b, 0);
    world.sync(a, 0);
    world.settle_and_assert();

    let workspace = world.devices[a].as_ref().unwrap().state.workspace.clone();
    assert!(
        workspace.folders.contains_key(&outer),
        "the outer folder was deleted by the merge"
    );
    assert!(
        workspace.folders.contains_key(&inner),
        "the inner folder was deleted by the merge"
    );
    assert!(
        workspace
            .schemes
            .values()
            .any(|scheme| scheme.name == "Inside"),
        "the scheme inside the moved folders was deleted"
    );
}

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

/// Trial (deep seed 10013): two devices carry the same line over into the same
/// existing Daily page before either syncs. The line's text must land once.
#[test]
fn a_line_carried_over_by_two_devices_at_once_keeps_its_text_once() {
    let mut world = world(90_040, 1);
    let a = world.add_device(Some(0));
    let yesterday = World::today() - Duration::days(1);
    let (source, line) = world.local(a, |device, _| {
        let day = open_day(device, yesterday);
        add_line(device, day, "carried once");
        let line = device
            .state
            .workspace
            .scheme(day)
            .unwrap()
            .items
            .last()
            .unwrap()
            .id;
        (day, line)
    });
    world.sync(a, 0);
    let b = world.add_device(Some(0));
    world.sync(b, 0);
    let c = world.add_device(Some(0));
    world.sync(c, 0);
    world.sync(a, 0);
    // Every device opens today and moves the same line into it, offline.
    let mut targets = Vec::new();
    for index in [a, b, c] {
        let target = world.local(index, |device, _| {
            let today = device.today();
            let target = open_day(device, today);
            let workspace = device.state.workspace.clone();
            let moved = workspace
                .scheme(source)
                .unwrap()
                .item(line)
                .unwrap()
                .clone();
            let position = workspace.scheme(target).map_or(0, |s| s.items.len());
            device
                .state
                .apply_command(Command::Batch(vec![
                    Command::DeleteItem {
                        scheme: source,
                        item: line,
                    },
                    Command::InsertItem {
                        scheme: target,
                        position,
                        item: moved,
                    },
                ]))
                .expect("carry the line over");
            target
        });
        targets.push(target);
    }
    assert_eq!(
        targets[0], targets[1],
        "both devices carry into the same day"
    );
    world.sync(a, 0);
    world.sync(b, 0);
    world.sync(a, 0);
    world.sync(b, 0);
    for index in [a, b] {
        let workspace = &world.devices[index].as_ref().unwrap().state.workspace;
        let texts: Vec<_> = workspace
            .schemes
            .values()
            .filter_map(|scheme| scheme.item(line).map(|item| (scheme.id, item.text())))
            .collect();
        assert_eq!(
            texts,
            vec![(targets[0], "carried once".to_string())],
            "device {index}: the carried line must hold its text once"
        );
    }
    world.settle_and_assert();
}
