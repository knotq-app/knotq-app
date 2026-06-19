use knotq_commands::{Command, WorkspaceCommandExt};
use knotq_model::{NodeRef, Scheme, Workspace};

mod support;

use support::{create_folder_named, create_scheme_named};

#[test]
fn file_like_node_names_are_allowed_without_sanitizing() {
    let mut workspace = Workspace::new();
    let root = workspace.root;

    let folder_id = create_folder_named(&mut workspace, root, " Bad/Folder? ");
    assert_eq!(workspace.folders[&folder_id].name, " Bad/Folder? ");

    let scheme_id = create_scheme_named(&mut workspace, root, "Notes.knotq", 0);
    assert_eq!(workspace.schemes[&scheme_id].name, "Notes.knotq");

    workspace
        .apply(Command::RenameScheme {
            id: scheme_id,
            name: "Plan: A/B? <draft>.knotq".into(),
        })
        .unwrap();
    assert_eq!(
        workspace.schemes[&scheme_id].name,
        "Plan: A/B? <draft>.knotq"
    );

    workspace
        .apply(Command::RenameFolder {
            id: folder_id,
            name: ".hidden".into(),
        })
        .unwrap();
    assert_eq!(workspace.folders[&folder_id].name, ".hidden");
}

#[test]
fn duplicate_folder_and_scheme_names_are_allowed() {
    let mut workspace = Workspace::new();
    let root = workspace.root;

    let first_folder = create_folder_named(&mut workspace, root, "Projects");
    let second_folder = create_folder_named(&mut workspace, root, "projects");
    assert_eq!(
        workspace.folders[&root].children,
        vec![
            NodeRef::Folder(first_folder),
            NodeRef::Folder(second_folder)
        ]
    );

    let first_scheme = create_scheme_named(&mut workspace, first_folder, "Notes", 0);
    let second_scheme = create_scheme_named(&mut workspace, first_folder, "notes", 0);
    assert_eq!(
        workspace.folders[&first_folder].children,
        vec![
            NodeRef::Scheme(first_scheme),
            NodeRef::Scheme(second_scheme)
        ]
    );
}

#[test]
fn rename_restore_and_move_can_create_duplicate_scheme_names() {
    let mut workspace = Workspace::new();
    let root = workspace.root;
    let source_folder = create_folder_named(&mut workspace, root, "Source");
    let target_folder = create_folder_named(&mut workspace, root, "Target");
    let source_id = create_scheme_named(&mut workspace, source_folder, "Plan", 0);
    let target_id = create_scheme_named(&mut workspace, target_folder, "Plan", 0);
    let other_id = create_scheme_named(&mut workspace, source_folder, "Other", 0);

    workspace
        .apply(Command::RenameScheme {
            id: other_id,
            name: "plan".into(),
        })
        .unwrap();
    assert_eq!(workspace.schemes[&other_id].name, "plan");

    workspace
        .apply(Command::MoveNode {
            node: NodeRef::Scheme(source_id),
            new_parent: target_folder,
            position: 1,
        })
        .unwrap();
    assert_eq!(
        workspace.folders[&target_folder].children,
        vec![NodeRef::Scheme(target_id), NodeRef::Scheme(source_id)]
    );

    let duplicate = Scheme::new("plan", 0);
    let duplicate_id = duplicate.id;
    workspace
        .apply(Command::RestoreScheme {
            folder: source_folder,
            position: 0,
            scheme: duplicate,
        })
        .unwrap();
    assert_eq!(
        workspace.folders[&source_folder].children,
        vec![NodeRef::Scheme(duplicate_id), NodeRef::Scheme(other_id)]
    );
}
