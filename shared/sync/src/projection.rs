//! The projection law: a device's plain [`Workspace`] is exactly what its own
//! CRDT documents materialize to.
//!
//! Two devices converge because Yrs merges their documents deterministically.
//! That guarantee only reaches the user if what the user sees *is* the
//! document. Whenever a plain workspace holds a value its own CRDT never did,
//! the next landing materializes the CRDT's value instead — and to every
//! observer (including the no-silent-loss oracle) that looks like a remote
//! change nobody made. Several of the hardest historical sync failures in
//! `app/TODO.md` are that shape.
//!
//! So the property is worth stating on its own, checkable on a single device
//! with no server and no second replica:
//!
//! ```text
//!     materialize(documents(device)) == workspace(device)
//! ```
//!
//! [`divergences`] returns the places where it does not hold, one line each,
//! naming the field rather than dumping two whole workspaces — the field name
//! is what identifies which writer is at fault.
//!
//! ## What is deliberately *not* a divergence
//!
//! A scheme whose document has never been populated. `from_states` refuses to
//! seed a document it has no bytes for (see its doc comment: seeding one under
//! a throwaway identity is what corrupts sync), so a scheme untouched since
//! the install legitimately lives only in plain storage until something
//! populates it. Those schemes are skipped and reported by
//! [`Divergences::unpopulated_schemes`].

use std::collections::HashSet;

use knotq_model::{NodeRef, SchemeId, Workspace};

use crate::crdt::WorkspaceCrdtDocuments;

/// The outcome of one projection check.
#[derive(Clone, Debug, Default)]
pub struct Divergences {
    /// One line per field where the plain workspace and the documents disagree.
    /// Empty means the law holds.
    pub lines: Vec<String>,
    /// Schemes excluded from the comparison because their document holds no
    /// population yet. Not a fault; see the module docs.
    pub unpopulated_schemes: HashSet<SchemeId>,
}

impl Divergences {
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// The divergences as one indented block, for a panic message.
    pub fn report(&self) -> String {
        self.lines.join("\n  ")
    }
}

/// Where `plain` disagrees with what `docs` materializes to.
///
/// Errors only if the documents cannot be materialized at all — itself a
/// failure, but a different one from a field disagreeing.
pub fn divergences(
    plain: &Workspace,
    docs: &WorkspaceCrdtDocuments,
) -> anyhow::Result<Divergences> {
    let unpopulated: HashSet<SchemeId> = plain
        .schemes
        .keys()
        .copied()
        .filter(|id| docs.scheme_document_is_unpopulated(*id))
        .collect();
    // Trust an empty document exactly for the schemes that *have* one: an
    // empty populated document means "every item was deleted", while an
    // unpopulated one means "not in the CRDT yet" and must not read as a wipe.
    let crdt = docs.materialized_workspace_repair(plain, &|id| !unpopulated.contains(id))?;
    let mut lines = Vec::new();
    compare_identity(plain, &crdt, &mut lines);
    compare_folders(plain, &crdt, &mut lines);
    compare_schemes(plain, &crdt, &unpopulated, &mut lines);
    compare_tree_state(plain, &crdt, &mut lines);
    Ok(Divergences {
        lines,
        unpopulated_schemes: unpopulated,
    })
}

fn compare_identity(plain: &Workspace, crdt: &Workspace, lines: &mut Vec<String>) {
    if plain.id != crdt.id {
        lines.push(format!("workspace id {:?} != {:?}", plain.id, crdt.id));
    }
    if plain.sync.id != crdt.sync.id {
        lines.push(format!(
            "workspace document id {:?} != {:?}",
            plain.sync.id, crdt.sync.id
        ));
    }
    if plain.root != crdt.root {
        lines.push(format!("root {:?} != {:?}", plain.root, crdt.root));
    }
}

fn compare_folders(plain: &Workspace, crdt: &Workspace, lines: &mut Vec<String>) {
    for (id, folder) in &plain.folders {
        let Some(other) = crdt.folders.get(id) else {
            lines.push(format!("folder {id:?} missing from the CRDT"));
            continue;
        };
        if folder.name != other.name {
            lines.push(format!(
                "folder {id:?} name {:?} != {:?}",
                folder.name, other.name
            ));
        }
        if folder.parent != other.parent {
            lines.push(format!(
                "folder {id:?} parent {:?} != {:?}",
                folder.parent, other.parent
            ));
        }
        if folder.expanded != other.expanded {
            lines.push(format!(
                "folder {id:?} expanded {} != {}",
                folder.expanded, other.expanded
            ));
        }
        if folder.children != other.children {
            // Name the archive status of the folder and of each child: the
            // materializer routes archived subtrees differently from live ones,
            // so which side of that boundary a node sits on is the first thing
            // to know when children disagree.
            let describe = |node: &NodeRef| match node {
                NodeRef::Scheme(scheme) => format!(
                    "{node:?}{}",
                    if plain.recently_deleted.contains(scheme) {
                        " [trashed]"
                    } else {
                        ""
                    }
                ),
                NodeRef::Folder(folder) => format!(
                    "{node:?}{}",
                    if plain.recently_deleted_folders.contains(folder) {
                        " [archived]"
                    } else {
                        ""
                    }
                ),
            };
            let render =
                |children: &[NodeRef]| children.iter().map(describe).collect::<Vec<_>>().join(", ");
            lines.push(format!(
                "folder {id:?}{} (parent {:?}) children [{}] != [{}]",
                if plain.recently_deleted_folders.contains(id) {
                    " [archived]"
                } else {
                    ""
                },
                folder.parent,
                render(&folder.children),
                render(&other.children),
            ));
        }
    }
    for id in crdt.folders.keys() {
        if !plain.folders.contains_key(id) {
            lines.push(format!("folder {id:?} only in the CRDT"));
        }
    }
}

fn compare_schemes(
    plain: &Workspace,
    crdt: &Workspace,
    unpopulated: &HashSet<SchemeId>,
    lines: &mut Vec<String>,
) {
    for (id, scheme) in &plain.schemes {
        if unpopulated.contains(id) {
            continue;
        }
        let Some(other) = crdt.schemes.get(id) else {
            lines.push(format!("scheme {id:?} missing from the CRDT"));
            continue;
        };
        if scheme.name != other.name {
            lines.push(format!(
                "scheme {id:?} name {:?} != {:?}",
                scheme.name, other.name
            ));
        }
        if scheme.color_index != other.color_index {
            lines.push(format!(
                "scheme {id:?} color {} != {}",
                scheme.color_index, other.color_index
            ));
        }
        if scheme.gsync != other.gsync {
            lines.push(format!(
                "scheme {id:?} gsync {} != {}",
                scheme.gsync, other.gsync
            ));
        }
        if scheme.source != other.source {
            lines.push(format!(
                "scheme {id:?} source {:?} != {:?}",
                scheme.source, other.source
            ));
        }
        if scheme.items.len() != other.items.len() {
            // Say *which* lines differ and, for one the CRDT placed in another
            // scheme, say where. A line the user sees here that the documents
            // put elsewhere is the cross-document duplicate-placement case
            // (`dedupe_materialized_items`), and naming the winning scheme is
            // what distinguishes it from an item that is simply absent.
            let present: HashSet<_> = other.items.iter().map(|item| item.id).collect();
            let elsewhere = |item: &knotq_model::ItemId| {
                crdt.schemes
                    .iter()
                    .find(|(other_id, other)| {
                        *other_id != id && other.items.iter().any(|line| line.id == *item)
                    })
                    .map(|(other_id, _)| format!(" (the CRDT places it in {other_id:?})"))
                    .unwrap_or_default()
            };
            let missing: Vec<_> = scheme
                .items
                .iter()
                .filter(|item| !present.contains(&item.id))
                .map(|item| format!("{:?}{}", item.id, elsewhere(&item.id)))
                .collect();
            let extra: Vec<_> = other
                .items
                .iter()
                .filter(|item| !scheme.items.iter().any(|line| line.id == item.id))
                .map(|item| format!("{:?}", item.id))
                .collect();
            lines.push(format!(
                "scheme {id:?} has {} item(s), the CRDT has {}; only in the workspace: [{}]; \
                 only in the CRDT: [{}]",
                scheme.items.len(),
                other.items.len(),
                missing.join(", "),
                extra.join(", "),
            ));
            continue;
        }
        for (left, right) in scheme.items.iter().zip(&other.items) {
            if left != right {
                lines.push(format!(
                    "scheme {id:?} item {:?} differs: {left:?} != {right:?}",
                    left.id
                ));
            }
        }
    }
    for id in crdt.schemes.keys() {
        if !plain.schemes.contains_key(id) {
            lines.push(format!("scheme {id:?} only in the CRDT"));
        }
    }
}

fn compare_tree_state(plain: &Workspace, crdt: &Workspace, lines: &mut Vec<String>) {
    if plain.daily_queue != crdt.daily_queue {
        lines.push(format!(
            "daily queue {:?} != {:?}",
            plain.daily_queue, crdt.daily_queue
        ));
    }
    if plain.recently_deleted != crdt.recently_deleted {
        lines.push(format!(
            "trash {:?} != {:?}",
            plain.recently_deleted, crdt.recently_deleted
        ));
    }
    if plain.recently_deleted_folders != crdt.recently_deleted_folders {
        lines.push(format!(
            "folder trash {:?} != {:?}",
            plain.recently_deleted_folders, crdt.recently_deleted_folders
        ));
    }
    for (id, meta) in &plain.scheme_sync {
        match crdt.scheme_sync.get(id) {
            Some(other) if other.id == meta.id => {}
            Some(other) => lines.push(format!(
                "scheme {id:?} bound to document {:?}, the CRDT says {:?}",
                meta.id, other.id
            )),
            None => lines.push(format!("scheme {id:?} has no CRDT binding")),
        }
    }
    for id in crdt.scheme_sync.keys() {
        if !plain.scheme_sync.contains_key(id) {
            lines.push(format!("scheme {id:?} bound only in the CRDT"));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use knotq_model::{Folder, FolderId, Item, ItemId, Scheme, SchemeId};

    /// A workspace with one folder holding one scheme with two lines, and the
    /// CRDT documents that exactly project it.
    fn workspace() -> (Workspace, FolderId, SchemeId) {
        let mut workspace = Workspace::new();
        let root = workspace.root;
        let folder_id = FolderId::new();
        workspace.folders.insert(
            folder_id,
            Folder {
                id: folder_id,
                name: "Work".to_string(),
                parent: Some(root),
                children: Vec::new(),
                expanded: true,
            },
        );
        workspace
            .folders
            .get_mut(&root)
            .expect("root")
            .children
            .push(NodeRef::Folder(folder_id));
        let mut scheme = Scheme::new("Plans", 3);
        scheme.items.push(Item::new("first"));
        scheme.items.push(Item::new("second"));
        let scheme_id = scheme.id;
        workspace.schemes.insert(scheme_id, scheme);
        workspace
            .folders
            .get_mut(&folder_id)
            .expect("folder")
            .children
            .push(NodeRef::Scheme(scheme_id));
        workspace.ensure_sync_metadata();
        (workspace, folder_id, scheme_id)
    }

    fn documents(workspace: &Workspace) -> WorkspaceCrdtDocuments {
        WorkspaceCrdtDocuments::try_new(workspace).expect("populate the documents")
    }

    /// `plain` against documents built from `workspace`: the caller edits one
    /// of the two so the difference is exactly what it wants to see reported.
    fn report(plain: &Workspace, documents: &WorkspaceCrdtDocuments) -> Vec<String> {
        divergences(plain, documents).expect("materialize").lines
    }

    #[test]
    fn a_workspace_matching_its_documents_reports_nothing() {
        let (workspace, _, _) = workspace();
        let docs = documents(&workspace);
        assert!(report(&workspace, &docs).is_empty());
    }

    #[test]
    fn a_scheme_field_the_documents_do_not_have_is_named() {
        let (workspace, _, scheme_id) = workspace();
        let docs = documents(&workspace);
        let mut plain = workspace.clone();
        let scheme = plain.schemes.get_mut(&scheme_id).expect("scheme");
        scheme.name = "Renamed".to_string();
        scheme.color_index = 7;
        scheme.gsync = !scheme.gsync;

        let lines = report(&plain, &docs);
        assert!(
            lines.iter().any(|line| line.contains("name")),
            "expected the name to be named: {lines:?}"
        );
        assert!(lines.iter().any(|line| line.contains("color")));
        assert!(lines.iter().any(|line| line.contains("gsync")));
    }

    #[test]
    fn a_line_the_documents_place_in_another_scheme_says_where() {
        let (workspace, folder_id, scheme_id) = workspace();
        // The documents hold the line in a second scheme; the plain workspace
        // still shows it in the first. This is the cross-document duplicate
        // placement case, and the report has to name the winning scheme.
        let mut in_documents = workspace.clone();
        let mut other = Scheme::new("Elsewhere", 0);
        let other_id = other.id;
        let moved = in_documents.schemes[&scheme_id].items[0].clone();
        other.items.push(moved.clone());
        in_documents
            .schemes
            .get_mut(&scheme_id)
            .expect("scheme")
            .items
            .retain(|item| item.id != moved.id);
        in_documents.schemes.insert(other_id, other);
        in_documents
            .folders
            .get_mut(&folder_id)
            .expect("folder")
            .children
            .push(NodeRef::Scheme(other_id));
        in_documents.ensure_sync_metadata();
        let docs = documents(&in_documents);

        let mut plain = in_documents.clone();
        plain
            .schemes
            .get_mut(&other_id)
            .expect("other")
            .items
            .clear();
        plain
            .schemes
            .get_mut(&scheme_id)
            .expect("scheme")
            .items
            .insert(0, moved.clone());

        let lines = report(&plain, &docs);
        let reported = lines.join("\n");
        assert!(
            reported.contains(&format!("{:?}", moved.id)),
            "expected the line to be named: {reported}"
        );
        assert!(
            reported.contains("the CRDT places it in"),
            "expected the winning scheme to be named: {reported}"
        );
    }

    /// Archive coherence, the shape `app/TODO.md` 0g describes: the documents
    /// say the scheme is archived (so the index keeps it out of the folder),
    /// while the plain workspace still lists it as a live child.
    #[test]
    fn a_folder_whose_children_differ_says_which_side_of_the_archive_each_is_on() {
        let (workspace, folder_id, scheme_id) = workspace();
        let mut in_documents = workspace.clone();
        in_documents.recently_deleted.push(scheme_id);
        in_documents
            .folders
            .get_mut(&folder_id)
            .expect("folder")
            .children
            .retain(|child| *child != NodeRef::Scheme(scheme_id));
        let docs = documents(&in_documents);

        // The plain workspace keeps the trashed scheme as a child anyway.
        let mut plain = in_documents.clone();
        plain
            .folders
            .get_mut(&folder_id)
            .expect("folder")
            .children
            .push(NodeRef::Scheme(scheme_id));

        let lines = report(&plain, &docs);
        let reported = lines.join("\n");
        assert!(
            reported.contains(&format!("{folder_id:?}")),
            "expected the folder to be named: {reported}"
        );
        assert!(
            reported.contains("[trashed]"),
            "expected the child's archive status: {reported}"
        );
    }

    #[test]
    fn a_scheme_only_the_documents_hold_is_reported() {
        let (workspace, folder_id, _) = workspace();
        let mut in_documents = workspace.clone();
        let mut extra = Scheme::new("Only in the CRDT", 0);
        extra.items.push(Item::new("a line"));
        let extra_id = extra.id;
        in_documents.schemes.insert(extra_id, extra);
        in_documents
            .folders
            .get_mut(&folder_id)
            .expect("folder")
            .children
            .push(NodeRef::Scheme(extra_id));
        in_documents.ensure_sync_metadata();
        let docs = documents(&in_documents);

        let mut plain = in_documents.clone();
        plain.schemes.remove(&extra_id);

        let lines = report(&plain, &docs);
        assert!(
            lines
                .iter()
                .any(|line| line.contains(&format!("{extra_id:?}"))),
            "expected the document-only scheme to be reported: {lines:?}"
        );
    }

    /// A scheme whose document was never populated is plain-only *by design*
    /// (`from_states` refuses to seed a document it has no bytes for), so it
    /// must not be reported as missing.
    #[test]
    fn a_locally_created_scheme_with_no_document_yet_is_not_a_divergence() {
        let (workspace, _, _) = workspace();
        let docs = documents(&workspace);
        let mut plain = workspace.clone();
        let fresh = Scheme::new("Just created", 0);
        let fresh_id = fresh.id;
        plain.schemes.insert(fresh_id, fresh);

        let found = divergences(&plain, &docs).expect("materialize");
        assert!(found.unpopulated_schemes.contains(&fresh_id));
        assert!(
            !found
                .lines
                .iter()
                .any(|line| line.contains(&format!("{fresh_id:?}"))),
            "{:?}",
            found.lines
        );
    }

    #[test]
    fn identity_and_tree_state_differences_are_reported() {
        let (workspace, _, scheme_id) = workspace();
        let docs = documents(&workspace);
        let mut plain = workspace.clone();
        plain.recently_deleted_folders.push(FolderId::new());
        plain.daily_queue.insert(
            chrono::NaiveDate::from_ymd_opt(2026, 9, 16).unwrap(),
            scheme_id,
        );

        let lines = report(&plain, &docs);
        let reported = lines.join("\n");
        assert!(reported.contains("folder trash"), "{reported}");
        assert!(reported.contains("daily queue"), "{reported}");
    }

    #[test]
    fn a_scheme_whose_document_was_never_populated_is_excluded_not_reported() {
        let (workspace, _, scheme_id) = workspace();
        // An empty document set: nothing is populated, so nothing is comparable
        // and the law must stay silent rather than read every scheme as lost.
        let docs = WorkspaceCrdtDocuments::from_states::<Vec<u8>>(
            &workspace,
            knotq_model::ReplicaId::new(),
            &HashMap::new(),
        )
        .expect("empty documents");
        let found = divergences(&workspace, &docs).expect("materialize");
        assert!(found.unpopulated_schemes.contains(&scheme_id));
        assert!(
            found.is_empty(),
            "an unpopulated document is not a divergence: {:?}",
            found.lines
        );
        assert!(found.report().is_empty());
    }

    #[test]
    fn an_item_field_difference_names_the_item() {
        let (workspace, _, scheme_id) = workspace();
        let docs = documents(&workspace);
        let mut plain = workspace.clone();
        let item: ItemId = plain.schemes[&scheme_id].items[1].id;
        plain
            .schemes
            .get_mut(&scheme_id)
            .expect("scheme")
            .items
            .get_mut(1)
            .expect("second line")
            .indent = 3;

        let lines = report(&plain, &docs);
        assert!(
            lines
                .iter()
                .any(|line| line.contains(&format!("{item:?}")) && line.contains("differs")),
            "{lines:?}"
        );
    }
}
