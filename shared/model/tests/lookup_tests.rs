//! The workspace's read side: the accessors every view and every sync path
//! calls before it decides anything.
//!
//! All pure tree walks, all previously uncovered. `subtree_scheme_ids` in
//! particular decides which schemes an archived folder takes with it, so its
//! cycle handling is not academic — a merged workspace really can contain a
//! folder cycle (see `normalize_one_level_folders`).

use chrono::NaiveDate;
use knotq_model::{Folder, FolderId, NodeRef, Scheme, Workspace};

fn folder(name: &str, parent: FolderId, children: Vec<NodeRef>) -> Folder {
    Folder {
        id: FolderId::new(),
        name: name.into(),
        parent: Some(parent),
        children,
        expanded: true,
    }
}

/// Build `root > outer > inner`, with one scheme in each of `outer` and
/// `inner`, and return the ids in that order.
fn nested_workspace() -> (Workspace, FolderId, FolderId) {
    let mut workspace = Workspace::new();
    let root = workspace.root;

    let outer_scheme = Scheme::new("Outer notes", 0);
    let inner_scheme = Scheme::new("Inner notes", 1);
    let (outer_scheme_id, inner_scheme_id) = (outer_scheme.id, inner_scheme.id);
    workspace.schemes.insert(outer_scheme_id, outer_scheme);
    workspace.schemes.insert(inner_scheme_id, inner_scheme);

    let inner = folder("Inner", root, vec![NodeRef::Scheme(inner_scheme_id)]);
    let inner_id = inner.id;
    let outer = folder(
        "Outer",
        root,
        vec![NodeRef::Scheme(outer_scheme_id), NodeRef::Folder(inner_id)],
    );
    let outer_id = outer.id;
    let mut inner = inner;
    inner.parent = Some(outer_id);

    workspace.folders.insert(outer_id, outer);
    workspace.folders.insert(inner_id, inner);
    workspace
        .folders
        .get_mut(&root)
        .unwrap()
        .children
        .push(NodeRef::Folder(outer_id));

    (workspace, outer_id, inner_id)
}

#[test]
fn folder_and_scheme_lookups_answer_for_present_ids_and_none_otherwise() {
    let (mut workspace, outer_id, _) = nested_workspace();
    let scheme_id = workspace.iter_schemes().next().unwrap().id;

    assert!(workspace.folder(outer_id).is_some());
    assert!(workspace.folder(FolderId::new()).is_none());
    assert!(workspace.scheme(scheme_id).is_some());
    assert!(workspace.scheme(Scheme::new("absent", 0).id).is_none());

    workspace.scheme_mut(scheme_id).unwrap().name = "renamed".into();
    assert_eq!(workspace.scheme(scheme_id).unwrap().name, "renamed");
    assert_eq!(workspace.iter_schemes().count(), 2);
}

#[test]
fn a_subtree_gathers_everything_below_it_and_nothing_beside_it() {
    let (workspace, outer_id, inner_id) = nested_workspace();

    assert_eq!(
        workspace.subtree_scheme_ids(outer_id).len(),
        2,
        "the outer folder owns its own scheme and the nested one"
    );
    assert_eq!(
        workspace.subtree_scheme_ids(inner_id).len(),
        1,
        "the inner folder owns only its own"
    );
    assert!(
        workspace.subtree_scheme_ids(workspace.root).len() >= 2,
        "walking from the root reaches everything"
    );

    let folders = workspace.subtree_folder_ids(outer_id);
    assert!(folders.contains(&outer_id) && folders.contains(&inner_id));
    assert!(!workspace.subtree_folder_ids(inner_id).contains(&outer_id));
}

#[test]
fn a_folder_cycle_does_not_hang_the_subtree_walks() {
    // Two devices can each move one of two folders into the other; only the
    // merged result holds the cycle, and these walks run on merged state.
    let (mut workspace, outer_id, inner_id) = nested_workspace();
    workspace
        .folders
        .get_mut(&inner_id)
        .unwrap()
        .children
        .push(NodeRef::Folder(outer_id));

    assert_eq!(workspace.subtree_scheme_ids(outer_id).len(), 2);
    assert_eq!(workspace.subtree_folder_ids(outer_id).len(), 2);
}

#[test]
fn the_path_to_a_node_runs_from_the_root_down_to_its_parent() {
    let (workspace, outer_id, inner_id) = nested_workspace();
    let inner_scheme = workspace.folders[&inner_id].children[0];

    assert_eq!(
        workspace.path_to(NodeRef::Folder(inner_id)),
        vec![workspace.root, outer_id],
        "the path runs down to the node's parent, exclusive of the node itself"
    );
    assert_eq!(
        workspace.path_to(inner_scheme),
        vec![workspace.root, outer_id, inner_id],
        "a scheme's path is the folders above it"
    );
    assert!(
        workspace
            .path_to(NodeRef::Folder(FolderId::new()))
            .is_empty(),
        "an absent node has no path"
    );
}

#[test]
fn daily_queue_bindings_are_readable_in_both_directions() {
    let mut workspace = Workspace::new();
    let date = NaiveDate::from_ymd_opt(2026, 9, 16).unwrap();
    let daily = Scheme::new("Daily 2026-09-16", 0);
    let daily_id = daily.id;
    workspace.schemes.insert(daily_id, daily);
    workspace.daily_queue.insert(date, daily_id);

    assert_eq!(workspace.daily_queue_scheme_id(date), Some(daily_id));
    assert_eq!(workspace.daily_queue_date_for_scheme(daily_id), Some(date));
    assert!(workspace.is_daily_queue_scheme(daily_id));

    let ordinary = Scheme::new("Notes", 0);
    assert!(!workspace.is_daily_queue_scheme(ordinary.id));
    assert_eq!(
        workspace.daily_queue_date_for_scheme(ordinary.id),
        None,
        "an unbound scheme belongs to no day"
    );
    assert_eq!(workspace.iter_daily_queue_schemes().count(), 1);
}
