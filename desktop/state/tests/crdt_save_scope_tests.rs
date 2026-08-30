//! Which CRDT document state files a save is allowed to skip.
//!
//! The narrowed save exists because rewriting every document means reading every
//! state file back to compare against, for an edit that touched one. The risk is
//! entirely one-sided: a scope that is too wide costs a little IO, a scope that
//! is too narrow leaves a document's state stale on disk — and a stale state
//! file is re-seeded from nothing on the next launch.
//!
//! So these tests are written the same way round. The invariant checked
//! everywhere is *never narrower than the truth*: every document whose encoded
//! state actually moved must be in the scope. Being wider than necessary is
//! allowed and only asserted where narrowing is the point.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use knotq_commands::{Command, CommandOrigin};
use knotq_model::{DocumentId, Item, ItemId, NodeRef, ReplicaId, Scheme, SchemeId, Workspace};
use knotq_state::{CrdtSaveScope, WorkspaceDirtyState, WorkspaceStore};
use knotq_sync::WorkspaceCrdtDocuments;

fn store_with_schemes(names: &[&str]) -> (WorkspaceStore, Vec<SchemeId>, Vec<ItemId>) {
    let mut workspace = Workspace::new();
    let mut schemes = Vec::new();
    let mut first_items = Vec::new();
    for name in names {
        let mut scheme = Scheme::new(*name, 0);
        let item = Item::new(format!("{name} line"));
        first_items.push(item.id);
        scheme.items.push(item);
        schemes.push(scheme.id);
        workspace
            .folders
            .get_mut(&workspace.root)
            .unwrap()
            .children
            .push(NodeRef::Scheme(scheme.id));
        workspace.schemes.insert(scheme.id, scheme);
    }
    workspace.ensure_sync_metadata();
    let seeded = WorkspaceCrdtDocuments::try_new(&workspace)
        .unwrap()
        .document_states();
    let store = WorkspaceStore::new(workspace, ReplicaId::new(), false, seeded, 1);
    (store, schemes, first_items)
}

/// Take the scope and settle the store into the steady state a running app is
/// in: one full save already done, so the next one can narrow.
fn settle(store: &mut WorkspaceStore) -> HashMap<DocumentId, Arc<[u8]>> {
    let (scope, _) = store.take_crdt_save_scope();
    assert_eq!(
        scope,
        CrdtSaveScope::All,
        "the first save of a run must write everything"
    );
    store.crdt_document_states()
}

/// The documents whose encoded state differs from `before` — the ground truth a
/// scope has to cover.
fn actually_changed(
    before: &HashMap<DocumentId, Arc<[u8]>>,
    after: &HashMap<DocumentId, Arc<[u8]>>,
) -> HashSet<DocumentId> {
    let mut changed = HashSet::new();
    for (document, bytes) in after {
        if before.get(document).map(|old| &old[..]) != Some(&bytes[..]) {
            changed.insert(*document);
        }
    }
    changed
}

fn assert_covers(scope: &CrdtSaveScope, changed: &HashSet<DocumentId>, label: &str) {
    match scope {
        CrdtSaveScope::All => {}
        CrdtSaveScope::Only(named) => {
            for document in changed {
                assert!(
                    named.contains(document),
                    "{label}: document {document} changed but the save would skip it, \
                     leaving it stale on disk"
                );
            }
        }
    }
}

fn edit_first_item(store: &mut WorkspaceStore, scheme: SchemeId, item: ItemId, text: &str) {
    store
        .apply_local(
            Command::UpdateItemText {
                scheme,
                item,
                text: text.to_string(),
            },
            CommandOrigin::User,
        )
        .unwrap();
}

/// The point of the whole thing: typing in one scheme of many must not schedule
/// a rewrite of the others.
#[test]
fn an_item_edit_names_only_the_scheme_it_touched() {
    let (mut store, schemes, items) = store_with_schemes(&["A", "B", "C"]);
    let before = settle(&mut store);

    edit_first_item(&mut store, schemes[1], items[1], "edited");

    let (scope, handles) = store.take_crdt_save_scope();
    let after = store.crdt_document_states();
    let changed = actually_changed(&before, &after);
    assert_covers(&scope, &changed, "item edit");

    let CrdtSaveScope::Only(named) = &scope else {
        panic!("an item edit must narrow the save, got {scope:?}");
    };
    assert_eq!(
        named, &changed,
        "the scope named documents that did not move"
    );
    assert_eq!(named.len(), 1, "one scheme was edited: {named:?}");
    assert_eq!(
        handles.keys().copied().collect::<HashSet<_>>(),
        *named,
        "the handles handed out must match the scope"
    );
}

/// Two edits between saves accumulate rather than the second replacing the first.
#[test]
fn edits_between_saves_accumulate() {
    let (mut store, schemes, items) = store_with_schemes(&["A", "B", "C"]);
    let before = settle(&mut store);

    edit_first_item(&mut store, schemes[0], items[0], "a1");
    edit_first_item(&mut store, schemes[2], items[2], "c1");

    let (scope, _) = store.take_crdt_save_scope();
    let changed = actually_changed(&before, &store.crdt_document_states());
    assert_covers(&scope, &changed, "two edits");
    let CrdtSaveScope::Only(named) = &scope else {
        panic!("item edits must narrow the save");
    };
    assert_eq!(
        named.len(),
        2,
        "both edited schemes must be named: {named:?}"
    );
}

/// Taking the scope resets it: a save that wrote everything must not make the
/// next one rewrite it again.
#[test]
fn taking_the_scope_clears_it() {
    let (mut store, schemes, items) = store_with_schemes(&["A", "B"]);
    settle(&mut store);
    edit_first_item(&mut store, schemes[0], items[0], "edited");
    let _ = store.take_crdt_save_scope();

    let (scope, handles) = store.take_crdt_save_scope();
    assert!(
        scope.is_empty(),
        "nothing changed since the last save: {scope:?}"
    );
    assert!(handles.is_empty());
}

/// A structural command can create or drop a scheme, and only a full save
/// sweeps the file of a document that went away.
#[test]
fn structural_commands_widen_the_scope() {
    for label in ["create scheme", "create folder"] {
        let (mut store, _, _) = store_with_schemes(&["A", "B"]);
        let before = settle(&mut store);
        let root = store.workspace().root;
        let command = match label {
            "create scheme" => Command::CreateScheme {
                folder: root,
                name: "New".into(),
                color_index: 0,
                position: None,
            },
            _ => Command::CreateFolder {
                parent: root,
                name: "Folder".into(),
                position: None,
            },
        };

        store.apply_local(command, CommandOrigin::User).unwrap();

        let (scope, _) = store.take_crdt_save_scope();
        let changed = actually_changed(&before, &store.crdt_document_states());
        assert_covers(&scope, &changed, label);
        assert_eq!(
            scope,
            CrdtSaveScope::All,
            "{label} changes the document set, so the save must be able to sweep"
        );
    }
}

/// Deleting a scheme drops its document. Its state file has to be swept, which
/// only the full save does.
#[test]
fn permanently_deleting_a_scheme_widens_the_scope() {
    let (mut store, schemes, _) = store_with_schemes(&["A", "B"]);
    // Archive first: a scheme is only purgeable out of the trash.
    store
        .apply_local(
            Command::DeleteScheme { id: schemes[1] },
            CommandOrigin::User,
        )
        .unwrap();
    let before = settle(&mut store);

    store
        .apply_local(
            Command::PermanentlyDeleteScheme { id: schemes[1] },
            CommandOrigin::User,
        )
        .unwrap();

    let (scope, _) = store.take_crdt_save_scope();
    let changed = actually_changed(&before, &store.crdt_document_states());
    assert_covers(&scope, &changed, "permanent delete");
    assert_eq!(
        scope,
        CrdtSaveScope::All,
        "a removed document's state file can only be swept by a full save"
    );
}

/// A wholesale rebuild replaces every document with a fresh object built from
/// bytes that need not match what is on disk.
#[test]
fn replacing_the_workspace_widens_the_scope() {
    let (mut store, schemes, items) = store_with_schemes(&["A", "B"]);
    let before = settle(&mut store);
    edit_first_item(&mut store, schemes[0], items[0], "edited");

    let workspace = store.workspace().clone();
    let dirty = WorkspaceDirtyState::all(&workspace);
    store.replace_workspace(workspace, dirty, false);

    let (scope, _) = store.take_crdt_save_scope();
    let changed = actually_changed(&before, &store.crdt_document_states());
    assert_covers(&scope, &changed, "replace_workspace");
    assert_eq!(scope, CrdtSaveScope::All);
}

/// A sync merge can land a remote update in any document, and the CRDT layer
/// does not report which ones moved.
#[test]
fn a_sync_merge_widens_the_scope() {
    let (mut store, schemes, items) = store_with_schemes(&["A", "B"]);
    let before = settle(&mut store);

    // A peer edits scheme B while this device sits idle.
    let mut remote_workspace = store.workspace().clone();
    let mut run_docs = WorkspaceCrdtDocuments::from_states(
        &remote_workspace,
        ReplicaId::new(),
        &store.crdt_document_states(),
    )
    .unwrap();
    remote_workspace.schemes.get_mut(&schemes[1]).unwrap().items[0].set_text("from a peer");
    let outcome = run_docs.sync_changes(
        &remote_workspace,
        &knotq_sync::WorkspaceCrdtChangeSet::default().touch_scheme(schemes[1]),
    );
    assert!(outcome.is_ok(), "{:?}", outcome.errors);

    assert!(store.merge_sync_crdt_states(&remote_workspace, &run_docs.document_states()));

    let (scope, _) = store.take_crdt_save_scope();
    let changed = actually_changed(&before, &store.crdt_document_states());
    assert!(
        !changed.is_empty(),
        "the merge was supposed to move a document"
    );
    assert_covers(&scope, &changed, "sync merge");
    assert_eq!(scope, CrdtSaveScope::All);
    let _ = items;
}

/// The sweep of an item edit's own scope must still cover an edit that lands
/// between the take and the next take — i.e. nothing is lost in the handover.
#[test]
fn an_edit_racing_the_take_lands_in_the_next_scope() {
    let (mut store, schemes, items) = store_with_schemes(&["A", "B"]);
    settle(&mut store);

    edit_first_item(&mut store, schemes[0], items[0], "first");
    let (first_scope, _) = store.take_crdt_save_scope();
    let after_first = store.crdt_document_states();

    edit_first_item(&mut store, schemes[0], items[0], "second");
    let (second_scope, _) = store.take_crdt_save_scope();
    let changed = actually_changed(&after_first, &store.crdt_document_states());

    assert_covers(&second_scope, &changed, "edit after the take");
    assert!(!first_scope.is_empty());
    assert!(
        !second_scope.is_empty(),
        "the second edit must be saved too"
    );
}

/// Long randomized runs over the mixed traffic a real session produces. The
/// invariant is the only one that matters: whatever moved is in the scope.
#[test]
fn a_random_session_never_produces_a_scope_that_is_too_narrow() {
    let mut seed = 0x2545_F491_4F6C_DD1Du64;
    let mut next = move || {
        seed ^= seed >> 12;
        seed ^= seed << 25;
        seed ^= seed >> 27;
        seed = seed.wrapping_mul(0x2545_F491_4F6C_DD1D);
        seed
    };

    let (mut store, schemes, items) = store_with_schemes(&["A", "B", "C", "D"]);
    let mut before = settle(&mut store);
    let mut live: Vec<(SchemeId, ItemId)> =
        schemes.iter().copied().zip(items.iter().copied()).collect();

    for step in 0..200u64 {
        match next() % 10 {
            // Item edits: the case that is allowed to narrow.
            0..=5 => {
                let (scheme, item) = live[(next() % live.len() as u64) as usize];
                edit_first_item(&mut store, scheme, item, &format!("step {step}"));
            }
            6 | 7 => {
                let (scheme, _) = live[(next() % live.len() as u64) as usize];
                let item = Item::new(format!("inserted {step}"));
                store
                    .apply_local(
                        Command::InsertItem {
                            scheme,
                            position: 0,
                            item,
                        },
                        CommandOrigin::User,
                    )
                    .unwrap();
            }
            // Structural: must widen.
            8 => {
                let parent = store.workspace().root;
                store
                    .apply_local(
                        Command::CreateScheme {
                            folder: parent,
                            name: format!("scheme {step}"),
                            color_index: 0,
                            position: None,
                        },
                        CommandOrigin::User,
                    )
                    .unwrap();
                // Adopt the new scheme so later steps can edit it too.
                if let Some((id, scheme)) = store
                    .workspace()
                    .schemes
                    .iter()
                    .find(|(_, scheme)| scheme.name == format!("scheme {step}"))
                {
                    if let Some(item) = scheme.items.first() {
                        live.push((*id, item.id));
                    }
                }
            }
            _ if live.len() > 1 => {
                let index = (next() % live.len() as u64) as usize;
                let (scheme, _) = live.remove(index);
                // Archive, then purge: two structural commands, and the purge is
                // the one that drops the document.
                store
                    .apply_local(Command::DeleteScheme { id: scheme }, CommandOrigin::User)
                    .unwrap();
                store
                    .apply_local(
                        Command::PermanentlyDeleteScheme { id: scheme },
                        CommandOrigin::User,
                    )
                    .unwrap();
            }
            _ => {}
        }

        // A save comes due every few steps, as the debounce makes it.
        if next() % 3 == 0 {
            let (scope, handles) = store.take_crdt_save_scope();
            let after = store.crdt_document_states();
            let changed = actually_changed(&before, &after);
            assert_covers(&scope, &changed, &format!("step {step}"));
            if let CrdtSaveScope::Only(named) = &scope {
                assert_eq!(
                    handles.keys().copied().collect::<HashSet<_>>(),
                    *named,
                    "step {step}: handles disagreed with the scope"
                );
            }
            before = after;
        }
    }
}
