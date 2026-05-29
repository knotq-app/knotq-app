use knotq_commands::{Command, CommandError, WorkspaceCommandExt};
use knotq_model::{FolderId, NodeRef, Scheme, SchemeId, Workspace};

#[test]
fn invalid_folder_and_scheme_names_are_rejected_without_sanitizing() {
    let mut workspace = Workspace::new();
    let root = workspace.root;

    let err = workspace
        .apply(Command::CreateFolder {
            parent: root,
            name: " Bad/Folder? ".into(),
            position: None,
        })
        .unwrap_err();
    assert!(matches!(err, CommandError::InvalidNodeName { .. }));
    assert_eq!(workspace.folders[&root].children, Vec::<NodeRef>::new());

    let err = workspace
        .apply(Command::CreateScheme {
            folder: root,
            name: "Notes.knotq".into(),
            color_index: 0,
            position: None,
        })
        .unwrap_err();
    assert!(matches!(err, CommandError::InvalidNodeName { .. }));
    assert_eq!(workspace.folders[&root].children, Vec::<NodeRef>::new());

    let scheme_id = create_scheme(&mut workspace, root, "Notes");
    let err = workspace
        .apply(Command::RenameScheme {
            id: scheme_id,
            name: "Plan: A/B? <draft>.knotq".into(),
        })
        .unwrap_err();
    assert!(matches!(err, CommandError::InvalidNodeName { .. }));
    assert_eq!(workspace.schemes[&scheme_id].name, "Notes");
}

#[test]
fn fully_illegal_names_are_rejected_without_sanitizing() {
    let mut workspace = Workspace::new();
    let root = workspace.root;

    let err = workspace
        .apply(Command::CreateFolder {
            parent: root,
            name: "..??".into(),
            position: None,
        })
        .unwrap_err();
    assert!(matches!(err, CommandError::InvalidNodeName { .. }));

    let scheme_id = create_scheme(&mut workspace, root, "Notes");
    let err = workspace
        .apply(Command::RenameScheme {
            id: scheme_id,
            name: "?.knotq".into(),
        })
        .unwrap_err();
    assert!(matches!(err, CommandError::InvalidNodeName { .. }));
    assert_eq!(workspace.schemes[&scheme_id].name, "Notes");
}

#[test]
fn folder_names_are_unique_at_the_root() {
    let mut workspace = Workspace::new();
    let root = workspace.root;
    create_folder(&mut workspace, root, "Projects");

    let err = workspace
        .apply(Command::CreateFolder {
            parent: root,
            name: "projects".into(),
            position: None,
        })
        .unwrap_err();
    assert!(matches!(err, CommandError::DuplicateFolderName { .. }));
}

#[test]
fn scheme_names_are_unique_within_a_folder_but_not_globally() {
    let mut workspace = Workspace::new();
    let root = workspace.root;
    let first_folder = create_folder(&mut workspace, root, "Work");
    let second_folder = create_folder(&mut workspace, root, "Personal");
    create_scheme(&mut workspace, first_folder, "Notes");
    create_scheme(&mut workspace, second_folder, "Notes");

    let err = workspace
        .apply(Command::CreateScheme {
            folder: first_folder,
            name: "notes".into(),
            color_index: 0,
            position: None,
        })
        .unwrap_err();
    assert!(matches!(err, CommandError::DuplicateSchemeName { .. }));
}

#[test]
fn rename_restore_and_move_cannot_create_duplicate_scheme_names() {
    let mut workspace = Workspace::new();
    let root = workspace.root;
    let source_folder = create_folder(&mut workspace, root, "Source");
    let target_folder = create_folder(&mut workspace, root, "Target");
    let source_id = create_scheme(&mut workspace, source_folder, "Plan");
    let target_id = create_scheme(&mut workspace, target_folder, "Plan");
    let other_id = create_scheme(&mut workspace, source_folder, "Other");

    let err = workspace
        .apply(Command::RenameScheme {
            id: other_id,
            name: "plan".into(),
        })
        .unwrap_err();
    assert!(matches!(err, CommandError::DuplicateSchemeName { .. }));
    assert_eq!(workspace.schemes[&other_id].name, "Other");

    let err = workspace
        .apply(Command::MoveNode {
            node: NodeRef::Scheme(source_id),
            new_parent: target_folder,
            position: 1,
        })
        .unwrap_err();
    assert!(matches!(err, CommandError::DuplicateSchemeName { .. }));
    assert_eq!(
        workspace.folders[&source_folder].children,
        vec![NodeRef::Scheme(source_id), NodeRef::Scheme(other_id)]
    );
    assert_eq!(
        workspace.folders[&target_folder].children,
        vec![NodeRef::Scheme(target_id)]
    );

    let duplicate = Scheme::new("Other", 0);
    let err = workspace
        .apply(Command::RestoreScheme {
            folder: source_folder,
            position: 0,
            scheme: duplicate,
        })
        .unwrap_err();
    assert!(matches!(err, CommandError::DuplicateSchemeName { .. }));
}

fn create_folder(workspace: &mut Workspace, parent: FolderId, name: &str) -> FolderId {
    let receipt = workspace
        .apply(Command::CreateFolder {
            parent,
            name: name.into(),
            position: None,
        })
        .unwrap();
    match receipt.inverse {
        Command::DeleteFolder { id } => id,
        _ => unreachable!(),
    }
}

fn create_scheme(workspace: &mut Workspace, folder: FolderId, name: &str) -> SchemeId {
    let receipt = workspace
        .apply(Command::CreateScheme {
            folder,
            name: name.into(),
            color_index: 0,
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
