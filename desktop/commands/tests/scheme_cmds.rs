use knotq_commands::{Command, WorkspaceCommandExt};
use knotq_model::{DeletedSchemeOrigin, NodeRef, Scheme, Workspace};

mod support;

use support::{create_folder, create_scheme};

#[test]
fn delete_scheme_removes_all_references_to_it() {
    let mut workspace = Workspace::new();
    let root = workspace.root;
    let scheme = Scheme::new("S", 1);
    let scheme_id = scheme.id;
    workspace.schemes.insert(scheme_id, scheme);
    workspace
        .folders
        .get_mut(&root)
        .unwrap()
        .children
        .extend([NodeRef::Scheme(scheme_id), NodeRef::Scheme(scheme_id)]);

    workspace
        .apply(Command::DeleteScheme { id: scheme_id })
        .unwrap();

    assert!(workspace.schemes.contains_key(&scheme_id));
    assert!(workspace.is_scheme_deleted(scheme_id));
    assert!(!workspace.folders[&root]
        .children
        .contains(&NodeRef::Scheme(scheme_id)));
}

#[test]
fn delete_scheme_records_restore_origin() {
    let mut workspace = Workspace::new();
    let root = workspace.root;
    let folder_id = create_folder(&mut workspace, root);
    let scheme_id = create_scheme(&mut workspace, folder_id);

    let receipt = workspace
        .apply(Command::DeleteScheme { id: scheme_id })
        .unwrap();

    assert!(workspace.is_scheme_deleted(scheme_id));
    assert_eq!(
        workspace.deleted_scheme_origin(scheme_id),
        Some(DeletedSchemeOrigin {
            folder: folder_id,
            position: 0,
        })
    );

    workspace.apply(receipt.inverse).unwrap();

    assert!(!workspace.is_scheme_deleted(scheme_id));
    assert_eq!(workspace.deleted_scheme_origin(scheme_id), None);
    assert_eq!(
        workspace.folders[&folder_id].children,
        vec![NodeRef::Scheme(scheme_id)]
    );
}

#[test]
fn permanently_delete_scheme_is_undoable_to_trash() {
    let mut workspace = Workspace::new();
    let root = workspace.root;
    let scheme_id = create_scheme(&mut workspace, root);

    workspace
        .apply(Command::DeleteScheme { id: scheme_id })
        .unwrap();
    let receipt = workspace
        .apply(Command::PermanentlyDeleteScheme { id: scheme_id })
        .unwrap();

    assert!(!workspace.schemes.contains_key(&scheme_id));
    assert!(!workspace.is_scheme_deleted(scheme_id));

    workspace.apply(receipt.inverse).unwrap();

    assert!(workspace.schemes.contains_key(&scheme_id));
    assert!(workspace.is_scheme_deleted(scheme_id));
    assert_eq!(
        workspace.deleted_scheme_origin(scheme_id),
        Some(DeletedSchemeOrigin {
            folder: root,
            position: 0,
        })
    );
}

/// Restoring a scheme into the folder it is already in must not be able to
/// index past the end of that folder.
///
/// `RestoreScheme` detaches the scheme from *every* folder before inserting it
/// at `position`, so the target's child list can be one shorter at the insert
/// than it was at the check. A scheme that is already the folder's only child,
/// restored at position 1, validated against a length of 1 and then inserted
/// into an empty list — a panic, and in the app a crash (production fuzz
/// single-account seeds 10041 and 10117).
#[test]
fn restoring_a_scheme_into_the_folder_it_already_sits_in_does_not_panic() {
    let mut workspace = Workspace::new();
    let root = workspace.root;
    let folder = create_folder(&mut workspace, root);
    let scheme_id = create_scheme(&mut workspace, folder);
    let scheme = workspace.schemes[&scheme_id].clone();
    assert_eq!(workspace.folders[&folder].children.len(), 1);

    // Position 1 is "after the one child" — which is this very scheme, so once
    // it is detached there is nothing to be after.
    let result = workspace.apply(Command::RestoreScheme {
        folder,
        position: 1,
        scheme: scheme.clone(),
    });

    assert!(result.is_ok(), "restoring in place must be accepted");
    assert_eq!(
        workspace.folders[&folder].children,
        vec![NodeRef::Scheme(scheme_id)],
        "and must leave exactly one copy of it in the folder"
    );

    // A position that is genuinely out of range is still rejected, and rejects
    // before anything is mutated.
    let before = workspace.clone();
    assert!(workspace
        .apply(Command::RestoreScheme {
            folder,
            position: 9,
            scheme,
        })
        .is_err());
    assert_eq!(
        workspace.folders[&folder].children, before.folders[&folder].children,
        "a rejected command leaves the workspace untouched"
    );
}

/// The `WorkspaceCommandExt` surface itself: the user-permission gate and the
/// `move_node` entry point the sidebar drags through.
#[test]
fn the_command_extension_gates_writes_to_a_read_only_scheme() {
    use knotq_model::{CalendarProvider, ImportedCalendarSource, Item, SchemeSource};

    let mut workspace = Workspace::new();
    let root = workspace.root;
    let scheme_id = create_scheme(&mut workspace, root);
    workspace.schemes.get_mut(&scheme_id).unwrap().source =
        SchemeSource::ImportedCalendar(ImportedCalendarSource {
            provider: CalendarProvider::Google,
            account_id: "acct".into(),
            account_email: None,
            calendar_id: "cal".into(),
            sync_token: None,
            read_only: true,
            last_synced_at: None,
        });

    let insert = Command::InsertItem {
        scheme: scheme_id,
        position: 0,
        item: Item::new("typed into a calendar feed"),
    };
    assert!(
        workspace.ensure_command_allowed_for_user(&insert).is_err(),
        "a read-only imported calendar must refuse a user edit"
    );

    // The same command is allowed once the scheme is the user's own.
    workspace.schemes.get_mut(&scheme_id).unwrap().source = SchemeSource::Local;
    assert!(workspace.ensure_command_allowed_for_user(&insert).is_ok());
    assert!(workspace.apply(insert).is_ok());
}

#[test]
fn the_command_extension_moves_a_node_between_folders() {
    let mut workspace = Workspace::new();
    let root = workspace.root;
    let source = create_folder(&mut workspace, root);
    let destination = create_folder(&mut workspace, root);
    let scheme_id = create_scheme(&mut workspace, source);

    let receipt = workspace
        .move_node(NodeRef::Scheme(scheme_id), destination, 0)
        .expect("moving a scheme between two real folders is allowed");
    assert!(receipt.touched.folders.contains(&destination));

    assert_eq!(
        workspace.folders[&destination].children,
        vec![NodeRef::Scheme(scheme_id)]
    );
    assert!(workspace.folders[&source].children.is_empty());

    // Applying the inverse puts it back, which is what undo relies on.
    workspace
        .apply(receipt.inverse)
        .expect("the inverse applies");
    assert_eq!(
        workspace.folders[&source].children,
        vec![NodeRef::Scheme(scheme_id)]
    );
}
