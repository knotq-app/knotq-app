use chrono::{TimeZone, Utc};
use knotq_commands::{Command, DateKind, WorkspaceCommandExt};
use knotq_model::{Item, ItemMarker, NodeRef, Workspace};

mod support;

use support::{create_folder, create_scheme, roundtrip, snapshot};

#[test]
fn apply_then_inverse_restores_folder_scheme_and_item_state() {
    let mut workspace = Workspace::new();
    let root = workspace.root;
    let original = snapshot(&workspace);
    roundtrip(
        &mut workspace,
        Command::CreateFolder {
            parent: root,
            name: "Projects".into(),
            position: None,
        },
        original,
    );

    let folder_id = create_folder(&mut workspace, root);
    let original = snapshot(&workspace);
    roundtrip(
        &mut workspace,
        Command::CreateScheme {
            folder: folder_id,
            name: "S".into(),
            color_index: 1,
            position: None,
        },
        original,
    );

    let scheme_id = create_scheme(&mut workspace, folder_id);
    let item = Item::new("hello");
    let item_id = item.id;
    workspace
        .apply(Command::InsertItem {
            scheme: scheme_id,
            position: 0,
            item,
        })
        .unwrap();
    let original = snapshot(&workspace);
    roundtrip(
        &mut workspace,
        Command::SetItemDate {
            scheme: scheme_id,
            item: item_id,
            kind: DateKind::Start,
            date: Some(Utc.with_ymd_and_hms(2026, 1, 1, 9, 0, 0).unwrap()),
        },
        original,
    );
}

#[test]
fn move_node_inverse_restores_original_order() {
    let mut workspace = Workspace::new();
    let root = workspace.root;
    let first = create_folder(&mut workspace, root);
    let second = create_folder(&mut workspace, root);
    let original = snapshot(&workspace);

    roundtrip(
        &mut workspace,
        Command::MoveNode {
            node: NodeRef::Folder(first),
            new_parent: root,
            position: 1,
        },
        original,
    );

    assert_eq!(
        workspace.folders[&root].children,
        vec![NodeRef::Folder(first), NodeRef::Folder(second)]
    );
}

#[test]
fn replace_item_inverse_restores_marker_constraints() {
    let mut workspace = Workspace::new();
    let root = workspace.root;
    let scheme_id = create_scheme(&mut workspace, root);
    let item = Item::new("dated")
        .with_start(Utc.with_ymd_and_hms(2026, 1, 1, 9, 0, 0).unwrap())
        .done();
    let item_id = item.id;
    workspace
        .apply(Command::InsertItem {
            scheme: scheme_id,
            position: 0,
            item,
        })
        .unwrap();

    let mut replacement = Item::new("plain");
    replacement.id = item_id;
    replacement.marker = ItemMarker::Bullet;
    let original = snapshot(&workspace);

    roundtrip(
        &mut workspace,
        Command::ReplaceItem {
            scheme: scheme_id,
            item: replacement,
        },
        original,
    );
}
