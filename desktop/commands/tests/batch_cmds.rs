use knotq_commands::{ChangeSet, Command, WorkspaceCommandExt};
use knotq_model::Workspace;

#[test]
fn batch_collects_inverses_in_reverse_order() {
    let mut workspace = Workspace::new();
    let root = workspace.root;
    let receipt = workspace
        .apply(Command::Batch(vec![
            Command::CreateFolder {
                parent: root,
                name: "A".into(),
                position: None,
            },
            Command::CreateFolder {
                parent: root,
                name: "B".into(),
                position: None,
            },
        ]))
        .unwrap();

    assert_eq!(workspace.folders[&root].children.len(), 2);

    let Command::Batch(inverses) = receipt.inverse else {
        panic!("expected batch inverse");
    };
    assert_eq!(inverses.len(), 2);

    workspace.apply(Command::Batch(inverses)).unwrap();

    assert!(workspace.folders[&root].children.is_empty());
}

#[test]
fn changeset_merge_deduplicates_touched_entities() {
    let mut first = ChangeSet::default().touched_folder(knotq_model::FolderId::new());
    let folder = first.folders[0];
    let scheme = knotq_model::SchemeId::new();
    first.merge(
        ChangeSet::default()
            .touched_folder(folder)
            .touched_scheme(scheme),
    );

    assert_eq!(first.folders, vec![folder]);
    assert_eq!(first.schemes, vec![scheme]);
}
