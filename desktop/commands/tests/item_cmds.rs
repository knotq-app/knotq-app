use chrono::Utc;
use knotq_commands::{Command, DateKind, WorkspaceCommandExt};
use knotq_model::{Item, ItemKind, ItemMarker, OccurrenceId, Workspace};

#[test]
fn create_and_toggle_item_with_undo() {
    let mut workspace = Workspace::new();
    let scheme_id = create_scheme(&mut workspace);
    let item = Item::new("hello");
    let item_id = item.id;
    workspace
        .apply(Command::InsertItem {
            scheme: scheme_id,
            position: 0,
            item,
        })
        .unwrap();

    let toggle = workspace
        .apply(Command::ToggleOccurrence {
            scheme: scheme_id,
            item: item_id,
            occurrence: OccurrenceId::Single,
        })
        .unwrap();

    assert!(workspace.schemes[&scheme_id].items[0].state[0]
        .state
        .is_done());

    workspace.apply(toggle.inverse).unwrap();

    assert!(!workspace.schemes[&scheme_id].items[0].state[0]
        .state
        .is_done());
}

#[test]
fn toggle_occurrence_promotes_non_checkbox_and_undo_restores_marker() {
    let mut workspace = Workspace::new();
    let scheme_id = create_scheme(&mut workspace);
    let mut item = Item::new("hello");
    item.marker = ItemMarker::Bullet;
    let item_id = item.id;
    workspace
        .apply(Command::InsertItem {
            scheme: scheme_id,
            position: 0,
            item,
        })
        .unwrap();

    let toggle = workspace
        .apply(Command::ToggleOccurrence {
            scheme: scheme_id,
            item: item_id,
            occurrence: OccurrenceId::Single,
        })
        .unwrap();

    let item = &workspace.schemes[&scheme_id].items[0];
    assert_eq!(item.marker, ItemMarker::Checkbox);
    assert!(item.state[0].state.is_done());

    workspace.apply(toggle.inverse).unwrap();

    let item = &workspace.schemes[&scheme_id].items[0];
    assert_eq!(item.marker, ItemMarker::Bullet);
    assert_eq!(item.state[0].state.progress, 0);
}

#[test]
fn set_occurrence_notification_offset_is_undoable() {
    let mut workspace = Workspace::new();
    let scheme_id = create_scheme(&mut workspace);
    let item = Item::new("hello").with_start(Utc::now());
    let item_id = item.id;
    workspace
        .apply(Command::InsertItem {
            scheme: scheme_id,
            position: 0,
            item,
        })
        .unwrap();

    let receipt = workspace
        .apply(Command::SetOccurrenceNotificationOffset {
            scheme: scheme_id,
            item: item_id,
            occurrence: OccurrenceId::Single,
            offset_secs: Some(-600),
        })
        .unwrap();

    assert_eq!(
        workspace.schemes[&scheme_id].items[0]
            .single_state()
            .notification_offset_secs,
        Some(-600)
    );

    workspace.apply(receipt.inverse).unwrap();

    assert_eq!(
        workspace.schemes[&scheme_id].items[0]
            .single_state()
            .notification_offset_secs,
        None
    );
}

#[test]
fn replace_item_is_undoable() {
    let mut workspace = Workspace::new();
    let scheme_id = create_scheme(&mut workspace);
    let dated = Item::new("").with_indent(2).with_start(Utc::now()).done();
    let item_id = dated.id;
    workspace
        .apply(Command::InsertItem {
            scheme: scheme_id,
            position: 0,
            item: dated,
        })
        .unwrap();

    let mut clean = Item::new("");
    clean.id = item_id;
    let replace = workspace
        .apply(Command::ReplaceItem {
            scheme: scheme_id,
            item: clean,
        })
        .unwrap();

    let item = &workspace.schemes[&scheme_id].items[0];
    assert_eq!(item.indent, 0);
    assert!(item.start.is_none());
    assert!(!item.state[0].state.is_done());

    workspace.apply(replace.inverse).unwrap();

    let item = &workspace.schemes[&scheme_id].items[0];
    assert_eq!(item.indent, 2);
    assert!(item.start.is_some());
    assert!(item.state[0].state.is_done());
}

#[test]
fn marker_constraints_clear_dates_for_non_checkbox_items() {
    let mut workspace = Workspace::new();
    let scheme_id = create_scheme(&mut workspace);
    let mut item = Item::new("plain").with_start(Utc::now());
    item.marker = ItemMarker::Blank;
    let item_id = item.id;
    workspace
        .apply(Command::InsertItem {
            scheme: scheme_id,
            position: 0,
            item,
        })
        .unwrap();

    let item = &workspace.schemes[&scheme_id].items[0];
    assert_eq!(item.id, item_id);
    assert_eq!(item.marker, ItemMarker::Blank);
    assert!(item.start.is_none());
    assert_eq!(item.kind(), ItemKind::Procedure);
}

#[test]
fn setting_date_promotes_non_checkbox_to_checkbox() {
    let mut workspace = Workspace::new();
    let scheme_id = create_scheme(&mut workspace);
    let mut item = Item::new("plain");
    item.marker = ItemMarker::Bullet;
    let item_id = item.id;
    workspace
        .apply(Command::InsertItem {
            scheme: scheme_id,
            position: 0,
            item,
        })
        .unwrap();

    let date = Utc::now();
    let receipt = workspace
        .apply(Command::SetItemDate {
            scheme: scheme_id,
            item: item_id,
            kind: DateKind::Start,
            date: Some(date),
        })
        .unwrap();

    let item = &workspace.schemes[&scheme_id].items[0];
    assert_eq!(item.marker, ItemMarker::Checkbox);
    assert_eq!(item.start, Some(date));

    workspace.apply(receipt.inverse).unwrap();

    let item = &workspace.schemes[&scheme_id].items[0];
    assert_eq!(item.marker, ItemMarker::Bullet);
    assert!(item.start.is_none());
}

#[test]
fn removing_checkbox_marker_clears_date_annotations_and_undo_restores() {
    let mut workspace = Workspace::new();
    let scheme_id = create_scheme(&mut workspace);
    let item = Item::new("dated").with_start(Utc::now()).done();
    let item_id = item.id;
    workspace
        .apply(Command::InsertItem {
            scheme: scheme_id,
            position: 0,
            item,
        })
        .unwrap();

    let receipt = workspace
        .apply(Command::SetItemMarker {
            scheme: scheme_id,
            item: item_id,
            marker: ItemMarker::Bullet,
        })
        .unwrap();

    let item = &workspace.schemes[&scheme_id].items[0];
    assert_eq!(item.marker, ItemMarker::Bullet);
    assert!(item.start.is_none());
    assert_eq!(item.kind(), ItemKind::Procedure);

    workspace.apply(receipt.inverse).unwrap();

    let item = &workspace.schemes[&scheme_id].items[0];
    assert_eq!(item.marker, ItemMarker::Checkbox);
    assert!(item.start.is_some());
}

fn create_scheme(workspace: &mut Workspace) -> knotq_model::SchemeId {
    let receipt = workspace
        .apply(Command::CreateScheme {
            folder: workspace.root,
            name: "S".into(),
            color_index: 1,
            position: None,
        })
        .unwrap();
    match receipt.inverse {
        Command::DeleteScheme { id } => id,
        Command::Batch(commands) => commands
            .into_iter()
            .find_map(|command| match command {
                Command::DeleteScheme { id } => Some(id),
                _ => None,
            })
            .unwrap(),
        _ => unreachable!(),
    }
}
