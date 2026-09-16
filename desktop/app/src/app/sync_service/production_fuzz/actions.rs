//! Random user actions, expressed the way the app expresses them: `Command`s
//! applied through the state layer, plus the few named entry points (a day
//! coming into being, carryover, paging days in, completing past events).

use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use knotq_commands::{Command, CommandOrigin, DateKind};
use knotq_model::{
    CalendarRecurrence, FolderId, ImageAssetFormat, ImageInline, Item, ItemId, ItemMarker,
    MarkerFamily, NodeRef, OccurrenceId, SchemeId, Table, Workspace,
};
use knotq_state::View;
use knotq_storage_json::{load_daily_queue_scheme, load_daily_queue_schemes_for_calendar_range};

use super::device::DesktopDevice;
use super::Rng;

const FAMILIES: [MarkerFamily; 10] = [
    MarkerFamily::Standard,
    MarkerFamily::Discs,
    MarkerFamily::Rings,
    MarkerFamily::Squares,
    MarkerFamily::Dashes,
    MarkerFamily::Alternating,
    MarkerFamily::Decimal,
    MarkerFamily::Alpha,
    MarkerFamily::Roman,
    MarkerFamily::Outline,
];

const MARKERS: [ItemMarker; 4] = [
    ItemMarker::Blank,
    ItemMarker::Bullet,
    ItemMarker::Numbered,
    ItemMarker::Checkbox,
];

fn pick<T: Copy>(rng: &mut Rng, values: &[T]) -> Option<T> {
    (!values.is_empty()).then(|| values[rng.below(values.len() as u64) as usize])
}

fn sorted<T: Ord + Copy>(values: impl Iterator<Item = T>) -> Vec<T> {
    let mut values: Vec<T> = values.collect();
    values.sort();
    values
}

/// Schemes the user can open and edit: live, not read-only.
fn editable_schemes(workspace: &Workspace) -> Vec<SchemeId> {
    sorted(workspace.schemes.keys().copied())
        .into_iter()
        .filter(|id| !workspace.is_scheme_deleted(*id) && !workspace.is_scheme_read_only(*id))
        .collect()
}

fn live_folders(workspace: &Workspace, include_root: bool) -> Vec<FolderId> {
    sorted(workspace.folders.keys().copied())
        .into_iter()
        .filter(|id| {
            (include_root || *id != workspace.root)
                && !workspace.is_folder_deleted(*id)
                && (*id == workspace.root
                    || workspace.folder(*id).is_some_and(|f| f.parent.is_some()))
        })
        .collect()
}

fn archived_schemes(workspace: &Workspace) -> Vec<SchemeId> {
    sorted(
        workspace
            .recently_deleted
            .iter()
            .copied()
            .filter(|id| workspace.schemes.contains_key(id)),
    )
}

fn archived_folders(workspace: &Workspace) -> Vec<FolderId> {
    sorted(
        workspace
            .recently_deleted_folders
            .iter()
            .copied()
            .filter(|id| workspace.folders.contains_key(id)),
    )
}

fn items_of(workspace: &Workspace, scheme: SchemeId) -> Vec<ItemId> {
    workspace
        .scheme(scheme)
        .map(|scheme| scheme.items.iter().map(|item| item.id).collect())
        .unwrap_or_default()
}

fn at(today: NaiveDate, hour_offset: i64) -> DateTime<Utc> {
    Utc.from_utc_datetime(&today.and_hms_opt(9, 0, 0).unwrap()) + Duration::hours(hour_offset)
}

fn random_item(device: &DesktopDevice, rng: &mut Rng) -> Item {
    let today = device.today();
    let label = rng.below(1_000_000);
    let mut item = Item::new(format!("line {label}"));
    match rng.below(10) {
        0 => item.marker = ItemMarker::Bullet,
        1 => item.marker = ItemMarker::Numbered,
        2 => item = item.with_marker(ItemMarker::Checkbox),
        // An event — possibly already over, so past-event completion has work.
        3 => {
            let start = at(today, rng.below(96) as i64 - 48);
            item = item.with_start(start).with_end(start + Duration::hours(1));
        }
        4 => item = item.with_start(at(today, rng.below(72) as i64 - 24)),
        5 => item = item.with_end(at(today, rng.below(72) as i64)),
        6 => {
            let start = at(today, rng.below(48) as i64 - 72);
            item = item
                .with_start(start)
                .with_end(start + Duration::minutes(30))
                .with_repeats(recurrence(rng));
        }
        7 => {
            let asset =
                uuid::Uuid::from_u128(u128::from(rng.below(u64::MAX)) << 64 | u128::from(label));
            let bytes = vec![(label % 251) as u8, 7, 7, 7, (label % 13) as u8];
            let path = device.image_dir.join(format!("{asset}.png"));
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&path, bytes);
            item.set_image(ImageInline {
                asset,
                format: ImageAssetFormat::Png,
                width: Some(16),
                height: Some(16),
            });
        }
        8 => item.set_table(Table::new(
            1 + rng.below(2) as usize,
            1 + rng.below(3) as usize,
        )),
        _ => {}
    }
    item.indent = rng.below(3) as u8;
    item
}

fn recurrence(rng: &mut Rng) -> CalendarRecurrence {
    let rule = match rng.below(3) {
        0 => "FREQ=DAILY",
        1 => "FREQ=WEEKLY",
        _ => "FREQ=DAILY;COUNT=5",
    };
    CalendarRecurrence {
        rrules: vec![rule.to_string()],
        rdates: Vec::new(),
        exdates: Vec::new(),
        overrides: Vec::new(),
        raw_import: None,
    }
}

/// Focus a scheme (or the union view) the way navigation does, which decides
/// the undo timeline.
fn focus(device: &mut DesktopDevice, scheme: Option<SchemeId>) {
    match scheme {
        Some(scheme) => {
            device.state.selection.view = View::Scheme;
            device.state.selection.scheme_id = Some(scheme);
        }
        None => {
            device.state.selection.view = View::Union;
            device.state.selection.scheme_id = None;
        }
    }
}

fn apply(device: &mut DesktopDevice, command: Command) -> bool {
    device.state.apply_command(command).is_some()
}

fn ensure_day(device: &mut DesktopDevice, date: NaiveDate) -> Option<SchemeId> {
    let path = device.workspace_path.clone();
    match device
        .state
        .ensure_daily_queue(date, || load_daily_queue_scheme(&path, date))
    {
        Ok((id, _)) => Some(id),
        Err((id, reason)) => {
            eprintln!("fuzz: daily queue {date}: {reason}");
            device.state.workspace.scheme(id).map(|_| id)
        }
    }
}

/// Perform one random local action on `device`. Returns a label for traces.
pub(super) fn random_local_action(device: &mut DesktopDevice, rng: &mut Rng) -> String {
    let workspace = device.state.workspace.clone();
    let today = device.today();
    let schemes = editable_schemes(&workspace);
    let scheme = pick(rng, &schemes);
    let items = scheme.map(|s| items_of(&workspace, s)).unwrap_or_default();
    let item = pick(rng, &items);
    let op = rng.below(48);
    if let Some(scheme) = scheme {
        if rng.below(3) != 0 {
            focus(device, Some(scheme));
        }
    }
    let name = |rng: &mut Rng, prefix: &str| format!("{prefix} {}", rng.below(10_000));

    let label = match op {
        0 | 1 => {
            let folders = live_folders(&workspace, true);
            let Some(folder) = pick(rng, &folders) else {
                return "skip".into();
            };
            let len = workspace.folders[&folder].children.len();
            let position = rng.below(len as u64 + 1) as usize;
            let scheme_name = name(rng, "scheme");
            apply(
                device,
                Command::CreateScheme {
                    folder,
                    name: scheme_name,
                    color_index: rng.below(18) as u8,
                    position: Some(position),
                },
            );
            "create scheme"
        }
        2 => {
            let parents = live_folders(&workspace, true);
            let parent = if rng.below(4) == 0 {
                pick(rng, &parents).unwrap_or(workspace.root)
            } else {
                workspace.root
            };
            let folder_name = name(rng, "folder");
            apply(
                device,
                Command::CreateFolder {
                    parent,
                    name: folder_name,
                    position: None,
                },
            );
            "create folder"
        }
        3 => {
            let Some(id) = scheme else {
                return "skip".into();
            };
            let new_name = name(rng, "renamed");
            apply(device, Command::RenameScheme { id, name: new_name });
            "rename scheme"
        }
        4 => {
            let Some(id) = scheme else {
                return "skip".into();
            };
            let color_index = rng.below(18) as u8;
            apply(device, Command::SetSchemeColor { id, color_index });
            "color scheme"
        }
        5 => {
            let Some(id) = pick(rng, &live_folders(&workspace, false)) else {
                return "skip".into();
            };
            let new_name = name(rng, "refolder");
            apply(device, Command::RenameFolder { id, name: new_name });
            "rename folder"
        }
        6 => {
            let Some(id) = pick(rng, &live_folders(&workspace, false)) else {
                return "skip".into();
            };
            let expanded = rng.below(2) == 0;
            apply(device, Command::SetFolderExpanded { id, expanded });
            "expand folder"
        }
        7 => {
            let Some(id) = scheme.filter(|id| !workspace.is_daily_queue_scheme(*id)) else {
                return "skip".into();
            };
            let Some(new_parent) = pick(rng, &live_folders(&workspace, true)) else {
                return "skip".into();
            };
            let len = workspace.folders[&new_parent].children.len();
            let position = rng.below(len as u64 + 1) as usize;
            apply(
                device,
                Command::MoveNode {
                    node: NodeRef::Scheme(id),
                    new_parent,
                    position,
                },
            );
            "move scheme"
        }
        8 => {
            let Some(folder) = pick(rng, &live_folders(&workspace, false)) else {
                return "skip".into();
            };
            let Some(new_parent) = pick(rng, &live_folders(&workspace, true)) else {
                return "skip".into();
            };
            let len = workspace.folders[&new_parent].children.len();
            let position = rng.below(len as u64 + 1) as usize;
            if std::env::var("KNOTQ_FUZZ_TRACE").is_ok() {
                eprintln!("[fuzz folder move] {folder} -> {new_parent} at {position}");
            }
            apply(
                device,
                Command::MoveNode {
                    node: NodeRef::Folder(folder),
                    new_parent,
                    position,
                },
            );
            "move folder"
        }
        9 => {
            let Some(id) = scheme.filter(|id| !workspace.is_daily_queue_scheme(*id)) else {
                return "skip".into();
            };
            focus(device, None);
            apply(device, Command::DeleteScheme { id });
            "archive scheme"
        }
        10 => {
            let Some(id) = pick(rng, &archived_schemes(&workspace)) else {
                return "skip".into();
            };
            let origin = workspace.deleted_scheme_origins.get(&id).copied();
            let (folder, position) = origin
                .filter(|origin| {
                    workspace.folders.contains_key(&origin.folder)
                        && !workspace.is_folder_deleted(origin.folder)
                })
                .map(|origin| (origin.folder, origin.position))
                .unwrap_or((workspace.root, 0));
            let len = workspace
                .folders
                .get(&folder)
                .map_or(0, |f| f.children.len());
            let scheme = workspace.schemes[&id].clone();
            apply(
                device,
                Command::RestoreScheme {
                    folder,
                    position: position.min(len),
                    scheme,
                },
            );
            "restore scheme"
        }
        11 => {
            let Some(id) = pick(rng, &archived_schemes(&workspace)) else {
                return "skip".into();
            };
            apply(device, Command::PermanentlyDeleteScheme { id });
            "permanently delete scheme"
        }
        12 => {
            let Some(id) = pick(rng, &live_folders(&workspace, false)) else {
                return "skip".into();
            };
            focus(device, None);
            apply(device, Command::DeleteFolder { id });
            "archive folder"
        }
        13 => {
            let Some(id) = pick(rng, &archived_folders(&workspace)) else {
                return "skip".into();
            };
            let origin = workspace.deleted_folder_origins.get(&id).copied();
            let (parent, position) = origin
                .filter(|origin| {
                    workspace.folders.contains_key(&origin.parent)
                        && !workspace.is_folder_deleted(origin.parent)
                })
                .map(|origin| (origin.parent, origin.position))
                .unwrap_or((workspace.root, 0));
            let len = workspace
                .folders
                .get(&parent)
                .map_or(0, |f| f.children.len());
            let folder = workspace.folders[&id].clone();
            apply(
                device,
                Command::RestoreFolder {
                    parent,
                    position: position.min(len),
                    folder,
                },
            );
            "restore folder"
        }
        14 => {
            let Some(id) = pick(rng, &archived_folders(&workspace)) else {
                return "skip".into();
            };
            apply(device, Command::PermanentlyDeleteFolder { id });
            "permanently delete folder"
        }
        15 => {
            // Empty the trash, as `workspace_ops::trash` does.
            let mut commands: Vec<Command> = archived_folders(&workspace)
                .into_iter()
                .map(|id| Command::PermanentlyDeleteFolder { id })
                .collect();
            let in_folders: std::collections::HashSet<SchemeId> = archived_folders(&workspace)
                .into_iter()
                .flat_map(|folder| workspace.subtree_scheme_ids(folder))
                .collect();
            commands.extend(
                archived_schemes(&workspace)
                    .into_iter()
                    .filter(|id| !in_folders.contains(id))
                    .map(|id| Command::PermanentlyDeleteScheme { id }),
            );
            let Some(command) = Command::from_vec(commands) else {
                return "skip".into();
            };
            apply(device, command);
            "empty trash"
        }
        16..=18 => {
            let Some(scheme) = scheme else {
                return "skip".into();
            };
            let position = rng.below(items.len() as u64 + 1) as usize;
            let new_item = random_item(device, rng);
            apply(
                device,
                Command::InsertItem {
                    scheme,
                    position,
                    item: new_item,
                },
            );
            "insert item"
        }
        19 | 20 => {
            // A typing burst: one editor command per keystroke.
            let (Some(scheme), Some(item)) = (scheme, item) else {
                return "skip".into();
            };
            let Some(mut text) = workspace
                .scheme(scheme)
                .and_then(|s| s.item(item))
                .and_then(|i| i.content.as_text().map(str::to_string))
            else {
                return "skip".into();
            };
            for _ in 0..1 + rng.below(6) {
                if rng.below(5) == 0 && !text.is_empty() {
                    text.pop();
                } else {
                    text.push((b'a' + rng.below(26) as u8) as char);
                }
                device.state.apply_editor_command(Command::UpdateItemText {
                    scheme,
                    item,
                    text: text.clone(),
                });
            }
            "type"
        }
        21 => {
            let (Some(scheme), Some(item)) = (scheme, item) else {
                return "skip".into();
            };
            apply(device, Command::DeleteItem { scheme, item });
            "delete item"
        }
        22 => {
            let Some(scheme) = scheme.filter(|_| items.len() >= 2) else {
                return "skip".into();
            };
            let from = rng.below(items.len() as u64) as usize;
            let to = rng.below(items.len() as u64) as usize;
            apply(device, Command::ReorderItem { scheme, from, to });
            "reorder item"
        }
        23 => {
            let (Some(scheme), Some(item)) = (scheme, item) else {
                return "skip".into();
            };
            apply(
                device,
                Command::SetItemIndent {
                    scheme,
                    item,
                    indent: rng.below(4) as u8,
                },
            );
            "indent"
        }
        24 => {
            let (Some(scheme), Some(item)) = (scheme, item) else {
                return "skip".into();
            };
            let marker = pick(rng, &MARKERS).unwrap();
            apply(
                device,
                Command::SetItemMarker {
                    scheme,
                    item,
                    marker,
                },
            );
            "marker"
        }
        25 => {
            let (Some(scheme), Some(item)) = (scheme, item) else {
                return "skip".into();
            };
            let family = pick(rng, &FAMILIES).unwrap();
            apply(
                device,
                Command::SetItemMarkerFamily {
                    scheme,
                    item,
                    family,
                },
            );
            "marker family"
        }
        26 | 27 => {
            // Move a date — including onto another day, which is how a user
            // reschedules on the calendar.
            let (Some(scheme), Some(item)) = (scheme, item) else {
                return "skip".into();
            };
            let kind = match rng.below(3) {
                0 => DateKind::Start,
                1 => DateKind::End,
                _ => DateKind::Available,
            };
            let date = (rng.below(5) != 0).then(|| at(today, rng.below(24 * 20) as i64 - 24 * 10));
            apply(
                device,
                Command::SetItemDate {
                    scheme,
                    item,
                    kind,
                    date,
                },
            );
            "set date"
        }
        28 => {
            let (Some(scheme), Some(item)) = (scheme, item) else {
                return "skip".into();
            };
            let repeats = (rng.below(3) != 0).then(|| recurrence(rng));
            apply(
                device,
                Command::SetItemRecurrence {
                    scheme,
                    item,
                    repeats,
                },
            );
            "recurrence"
        }
        29 | 30 => {
            let (Some(scheme), Some(item)) = (scheme, item) else {
                return "skip".into();
            };
            let Some(current) = workspace.scheme(scheme).and_then(|s| s.item(item)) else {
                return "skip".into();
            };
            let occurrence = match (current.repeats.is_some(), current.start) {
                (true, Some(start)) => {
                    OccurrenceId::recurring_utc(start + Duration::days(rng.below(4) as i64))
                }
                _ => OccurrenceId::Single,
            };
            apply(
                device,
                Command::ToggleOccurrence {
                    scheme,
                    item,
                    occurrence,
                },
            );
            "toggle done"
        }
        31 => {
            let (Some(scheme), Some(item)) = (scheme, item) else {
                return "skip".into();
            };
            let offset_secs = (rng.below(2) == 0).then(|| rng.below(3600) as i64);
            apply(
                device,
                Command::SetOccurrenceNotificationOffset {
                    scheme,
                    item,
                    occurrence: OccurrenceId::Single,
                    offset_secs,
                },
            );
            "notification offset"
        }
        32 => {
            let (Some(scheme), Some(item)) = (scheme, item) else {
                return "skip".into();
            };
            let priority = (rng.below(3) != 0).then(|| rng.below(5) as u8);
            apply(
                device,
                Command::SetItemPriority {
                    scheme,
                    item,
                    priority,
                },
            );
            "priority"
        }
        33 => {
            // Paste over a line: new content, same identity.
            let (Some(scheme), Some(item)) = (scheme, item) else {
                return "skip".into();
            };
            let mut replacement = random_item(device, rng);
            replacement.id = item;
            apply(
                device,
                Command::ReplaceItem {
                    scheme,
                    item: replacement,
                },
            );
            "replace item"
        }
        34 => {
            // Multi-line select + delete.
            let Some(scheme) = scheme.filter(|_| items.len() >= 2) else {
                return "skip".into();
            };
            let count = 2 + rng.below(items.len() as u64 - 1) as usize;
            let commands = items
                .iter()
                .take(count)
                .map(|item| Command::DeleteItem {
                    scheme,
                    item: *item,
                })
                .collect();
            apply(device, Command::Batch(commands));
            "delete lines"
        }
        35 => {
            // Multi-line paste.
            let Some(scheme) = scheme else {
                return "skip".into();
            };
            let start = rng.below(items.len() as u64 + 1) as usize;
            let commands = (0..2 + rng.below(3) as usize)
                .map(|offset| Command::InsertItem {
                    scheme,
                    position: start + offset,
                    item: random_item(device, rng),
                })
                .collect();
            apply(device, Command::Batch(commands));
            "paste lines"
        }
        36 | 37 => {
            // Move a line to another scheme — including today's Daily Queue,
            // the event popup's "Daily" target.
            let (Some(source), Some(item)) = (scheme, item) else {
                return "skip".into();
            };
            let target = if op == 37 {
                ensure_day(device, today)
            } else {
                pick(rng, &schemes)
            };
            let Some(target) = target.filter(|target| *target != source) else {
                return "skip".into();
            };
            let workspace = device.state.workspace.clone();
            let Some(moved) = workspace.scheme(source).and_then(|s| s.item(item)).cloned() else {
                return "skip".into();
            };
            let position = workspace.scheme(target).map_or(0, |s| s.items.len());
            if std::env::var("KNOTQ_FUZZ_TRACE").is_ok() {
                eprintln!("[fuzz move] {source} {item} -> {target} at {position}");
            }
            apply(
                device,
                Command::Batch(vec![
                    Command::DeleteItem {
                        scheme: source,
                        item,
                    },
                    Command::InsertItem {
                        scheme: target,
                        position,
                        item: moved,
                    },
                ]),
            );
            "move line to scheme"
        }
        38 => {
            let scope = if rng.below(2) == 0 { scheme } else { None };
            focus(device, scope);
            device.state.undo_command();
            "undo"
        }
        39 => {
            let scope = if rng.below(2) == 0 { scheme } else { None };
            focus(device, scope);
            device.state.redo_command();
            "redo"
        }
        40 => {
            ensure_day(device, today);
            "open today"
        }
        41 => {
            // Scroll the Daily Queue / navigate to a nearby day.
            let date =
                today - Duration::days(rng.below(20) as i64) + Duration::days(rng.below(3) as i64);
            ensure_day(device, date);
            "open a day"
        }
        42 => {
            // `KnotQApp::carryover_daily_queue`.
            let Some(previous_date) = knotq_state::last_nonempty_daily_queue_day(&workspace, today)
            else {
                return "skip".into();
            };
            let Some(previous_id) = workspace.daily_queue_scheme_id(previous_date) else {
                return "skip".into();
            };
            let Some(today_id) = ensure_day(device, today) else {
                return "skip".into();
            };
            let workspace = device.state.workspace.clone();
            let (Some(previous), Some(today_scheme)) =
                (workspace.scheme(previous_id), workspace.scheme(today_id))
            else {
                return "skip".into();
            };
            let Some(command) = knotq_state::daily_queue_carryover_command(
                previous_id,
                previous_date,
                previous,
                today_id,
                today_scheme,
            ) else {
                return "skip".into();
            };
            apply(device, command);
            "carry over"
        }
        43 => {
            // The calendar paging a month of days in
            // (`ensure_daily_queue_calendar_range_loaded`).
            let start = today - Duration::days(35);
            let Ok(loaded) =
                load_daily_queue_schemes_for_calendar_range(&device.workspace_path, start, today)
            else {
                return "skip".into();
            };
            let schemes = loaded
                .into_iter()
                .filter(|(date, scheme)| workspace.daily_queue_scheme_id(*date) == Some(scheme.id))
                .map(|(_, scheme)| scheme)
                .collect();
            device.state.adopt_loaded_schemes(schemes);
            "page days in"
        }
        44 => {
            knotq_state::complete_past_events(&mut device.state, Utc::now());
            "complete past events"
        }
        45 => {
            // Midnight: the device's day moves on and today's page appears.
            let next = today + Duration::days(1);
            device.set_today(next);
            ensure_day(device, next);
            "day rolls over"
        }
        46 => {
            // An MCP agent editing on the user's behalf.
            let (Some(scheme), Some(item)) = (scheme, item) else {
                return "skip".into();
            };
            let text = name(rng, "agent");
            let _ = device.state.apply_prechecked_local_command(
                Command::UpdateItemText { scheme, item, text },
                CommandOrigin::Agent,
            );
            "agent edit"
        }
        _ => {
            let Some(id) = scheme else {
                return "skip".into();
            };
            let gsync = rng.below(2) == 0;
            apply(device, Command::SetSchemeGsync { id, on: gsync });
            "gsync toggle"
        }
    };
    label.to_string()
}
