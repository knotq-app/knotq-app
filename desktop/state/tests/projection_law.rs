//! The projection law: a device's plain `Workspace` is exactly what its own
//! CRDT documents materialize to.
//!
//! Why this is the invariant worth pinning
//! ---------------------------------------
//! Two devices converge because Yrs merges their documents deterministically.
//! That guarantee only reaches the user if what the user sees *is* the
//! document. Every "a field reverted and no other device wrote it" failure in
//! `app/TODO.md` has the same shape underneath: the plain workspace held a
//! value its own CRDT never did, so the first landing that materialized from
//! the CRDT looked — correctly, from the CRDT's point of view — like a remote
//! change back to the old value.
//!
//! So the property checked here needs no server, no second device and no
//! network: after any sequence of commands on one store,
//!
//! ```text
//!     materialize(crdt_documents(store)) == store.workspace()
//! ```
//!
//! A violation is a local corruption that has not been noticed yet, and it is
//! reproducible from a seed in milliseconds rather than from a multi-device
//! fuzz run in minutes.
//!
//! The states are round-tripped through `document_states()` /`from_states`
//! rather than read off the live documents, so a divergence that only exists
//! in the *persisted* encoding (the form a sync pushes and a relaunch reloads)
//! fails here too.
//!
//! `KNOTQ_PROJECTION_SEEDS` / `KNOTQ_PROJECTION_STEPS` widen a run;
//! `KNOTQ_PROJECTION_TRACE=1` prints each applied command.

use std::collections::HashMap;

use chrono::NaiveDate;
use knotq_commands::{Command, CommandOrigin, DateKind};
use knotq_model::{FolderId, Item, ItemId, ItemMarker, NodeRef, Scheme, SchemeId, Workspace};
use knotq_state::WorkspaceStore;
use knotq_sync::WorkspaceCrdtDocuments;

/// SplitMix64, the same generator the production fuzzer uses, so a seed means
/// the same thing when a failure is carried between the two harnesses.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed ^ 0xD1B5_4A32_D192_ED03)
    }

    fn below(&mut self, bound: u64) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        if bound == 0 {
            0
        } else {
            z % bound
        }
    }

    fn pick<T: Copy>(&mut self, values: &[T]) -> Option<T> {
        if values.is_empty() {
            None
        } else {
            Some(values[self.below(values.len() as u64) as usize])
        }
    }
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn today() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 9, 15).expect("valid calendar date")
}

/// Schemes the user can edit: everything not in the trash.
fn live_schemes(workspace: &Workspace) -> Vec<SchemeId> {
    let mut ids: Vec<_> = workspace
        .schemes
        .keys()
        .copied()
        .filter(|id| !workspace.recently_deleted.contains(id))
        .collect();
    ids.sort();
    ids
}

fn live_folders(workspace: &Workspace) -> Vec<FolderId> {
    let mut ids: Vec<_> = workspace
        .folders
        .keys()
        .copied()
        .filter(|id| !workspace.recently_deleted_folders.contains(id))
        .collect();
    ids.sort();
    ids
}

fn items_of(workspace: &Workspace, scheme: SchemeId) -> Vec<ItemId> {
    workspace
        .schemes
        .get(&scheme)
        .map(|scheme| scheme.items.iter().map(|item| item.id).collect())
        .unwrap_or_default()
}

/// A starter-shaped workspace: a couple of folders and schemes with content, so
/// the first commands have something to act on instead of spending the run
/// creating it.
fn seed_workspace() -> Workspace {
    let mut workspace = Workspace::new();
    let root = workspace.root;
    for index in 0..2 {
        let folder_id = FolderId::new();
        workspace.folders.insert(
            folder_id,
            knotq_model::Folder {
                id: folder_id,
                name: format!("folder {index}"),
                parent: Some(root),
                children: Vec::new(),
                expanded: true,
            },
        );
        workspace
            .folders
            .get_mut(&root)
            .expect("root folder")
            .children
            .push(NodeRef::Folder(folder_id));
        for scheme_index in 0..2 {
            let mut scheme = Scheme::new(format!("scheme {index}-{scheme_index}"), 0);
            for line in 0..3 {
                scheme
                    .items
                    .push(Item::new(format!("line {index}-{scheme_index}-{line}")));
            }
            let scheme_id = scheme.id;
            workspace.schemes.insert(scheme_id, scheme);
            workspace
                .folders
                .get_mut(&folder_id)
                .expect("folder")
                .children
                .push(NodeRef::Scheme(scheme_id));
        }
    }
    workspace.ensure_sync_metadata();
    workspace
}

/// Apply one random command. Returns a label for the trace; commands the
/// invariant checker cannot act on (no scheme exists yet, say) report "skip".
fn random_command(store: &mut WorkspaceStore, rng: &mut Rng) -> String {
    let workspace = store.workspace().clone();
    let schemes = live_schemes(&workspace);
    let scheme = rng.pick(&schemes);
    let items = scheme
        .map(|id| items_of(&workspace, id))
        .unwrap_or_default();
    let item = rng.pick(&items);
    let folders = live_folders(&workspace);
    let name = |rng: &mut Rng, prefix: &str| format!("{prefix} {}", rng.below(10_000));

    let (label, command) = match rng.below(25) {
        0 => {
            let Some(folder) = rng.pick(&folders) else {
                return "skip".into();
            };
            let position = workspace.folders[&folder].children.len();
            (
                "create scheme",
                Command::CreateScheme {
                    folder,
                    name: name(rng, "scheme"),
                    color_index: rng.below(18) as u8,
                    position: Some(position),
                },
            )
        }
        1 => {
            let parent = rng.pick(&folders).unwrap_or(workspace.root);
            (
                "create folder",
                Command::CreateFolder {
                    parent,
                    name: name(rng, "folder"),
                    position: None,
                },
            )
        }
        2 => {
            let Some(id) = scheme else {
                return "skip".into();
            };
            (
                "rename scheme",
                Command::RenameScheme {
                    id,
                    name: name(rng, "renamed"),
                },
            )
        }
        3 => {
            let Some(id) = scheme else {
                return "skip".into();
            };
            (
                "recolor scheme",
                Command::SetSchemeColor {
                    id,
                    color_index: rng.below(18) as u8,
                },
            )
        }
        4 => {
            let Some(id) = rng.pick(&folders) else {
                return "skip".into();
            };
            if id == workspace.root {
                return "skip".into();
            }
            (
                "rename folder",
                Command::RenameFolder {
                    id,
                    name: name(rng, "folder"),
                },
            )
        }
        5 => {
            let Some(id) = rng.pick(&folders) else {
                return "skip".into();
            };
            (
                "expand folder",
                Command::SetFolderExpanded {
                    id,
                    expanded: rng.below(2) == 0,
                },
            )
        }
        6 => {
            let Some(id) = scheme else {
                return "skip".into();
            };
            let Some(new_parent) = rng.pick(&folders) else {
                return "skip".into();
            };
            let position = rng.below(workspace.folders[&new_parent].children.len() as u64 + 1);
            (
                "move scheme",
                Command::MoveNode {
                    node: NodeRef::Scheme(id),
                    new_parent,
                    position: position as usize,
                },
            )
        }
        7 => {
            let movable: Vec<_> = folders
                .iter()
                .copied()
                .filter(|id| *id != workspace.root)
                .collect();
            let (Some(node), Some(new_parent)) = (rng.pick(&movable), rng.pick(&folders)) else {
                return "skip".into();
            };
            if node == new_parent || is_ancestor(&workspace, node, new_parent) {
                return "skip".into();
            }
            let position = rng.below(workspace.folders[&new_parent].children.len() as u64 + 1);
            (
                "move folder",
                Command::MoveNode {
                    node: NodeRef::Folder(node),
                    new_parent,
                    position: position as usize,
                },
            )
        }
        8 => {
            let Some(id) = scheme else {
                return "skip".into();
            };
            ("delete scheme", Command::DeleteScheme { id })
        }
        9 => {
            let Some(id) = rng.pick(&workspace.recently_deleted) else {
                return "skip".into();
            };
            let Some(scheme) = workspace.schemes.get(&id).cloned() else {
                return "skip".into();
            };
            (
                "restore scheme",
                Command::RestoreScheme {
                    folder: workspace.root,
                    position: workspace.folders[&workspace.root].children.len(),
                    scheme,
                },
            )
        }
        10 => {
            let Some(id) = rng.pick(&workspace.recently_deleted) else {
                return "skip".into();
            };
            (
                "permanently delete scheme",
                Command::PermanentlyDeleteScheme { id },
            )
        }
        11 => {
            let deletable: Vec<_> = folders
                .iter()
                .copied()
                .filter(|id| *id != workspace.root)
                .collect();
            let Some(id) = rng.pick(&deletable) else {
                return "skip".into();
            };
            ("delete folder", Command::DeleteFolder { id })
        }
        12 => {
            let Some(id) = rng.pick(&workspace.recently_deleted_folders) else {
                return "skip".into();
            };
            let Some(folder) = workspace.folders.get(&id).cloned() else {
                return "skip".into();
            };
            (
                "restore folder",
                Command::RestoreFolder {
                    parent: workspace.root,
                    position: workspace.folders[&workspace.root].children.len(),
                    folder,
                },
            )
        }
        13 => {
            let Some(scheme) = scheme else {
                return "skip".into();
            };
            let position = rng.below(items.len() as u64 + 1) as usize;
            (
                "insert item",
                Command::InsertItem {
                    scheme,
                    position,
                    item: Item::new(name(rng, "line")),
                },
            )
        }
        14 => {
            let (Some(scheme), Some(item)) = (scheme, item) else {
                return "skip".into();
            };
            (
                "update text",
                Command::UpdateItemText {
                    scheme,
                    item,
                    text: name(rng, "text"),
                },
            )
        }
        15 => {
            let (Some(scheme), Some(item)) = (scheme, item) else {
                return "skip".into();
            };
            ("delete item", Command::DeleteItem { scheme, item })
        }
        16 => {
            let Some(scheme) = scheme else {
                return "skip".into();
            };
            if items.len() < 2 {
                return "skip".into();
            }
            let from = rng.below(items.len() as u64) as usize;
            let to = rng.below(items.len() as u64) as usize;
            ("reorder item", Command::ReorderItem { scheme, from, to })
        }
        17 => {
            let (Some(scheme), Some(item)) = (scheme, item) else {
                return "skip".into();
            };
            (
                "indent item",
                Command::SetItemIndent {
                    scheme,
                    item,
                    indent: rng.below(4) as u8,
                },
            )
        }
        18 => {
            let (Some(scheme), Some(item)) = (scheme, item) else {
                return "skip".into();
            };
            let marker = match rng.below(3) {
                0 => ItemMarker::Blank,
                1 => ItemMarker::Checkbox,
                _ => ItemMarker::Bullet,
            };
            (
                "set marker",
                Command::SetItemMarker {
                    scheme,
                    item,
                    marker,
                },
            )
        }
        19 => {
            let (Some(scheme), Some(item)) = (scheme, item) else {
                return "skip".into();
            };
            let kind = match rng.below(3) {
                0 => DateKind::Start,
                1 => DateKind::End,
                _ => DateKind::Available,
            };
            let date = today()
                .and_hms_opt(9, 0, 0)
                .expect("valid time")
                .and_utc()
                .checked_add_signed(chrono::Duration::hours(rng.below(96) as i64));
            (
                "set date",
                Command::SetItemDate {
                    scheme,
                    item,
                    kind,
                    date,
                },
            )
        }
        20 => {
            let (Some(scheme), Some(item)) = (scheme, item) else {
                return "skip".into();
            };
            (
                "set priority",
                Command::SetItemPriority {
                    scheme,
                    item,
                    priority: (rng.below(4) as u8).checked_sub(1),
                },
            )
        }
        21 => {
            let Some(scheme) = scheme else {
                return "skip".into();
            };
            let count = rng.below(3) + 1;
            let commands: Vec<_> = (0..count)
                .map(|offset| Command::InsertItem {
                    scheme,
                    position: offset as usize,
                    item: Item::new(name(rng, "batched")),
                })
                .collect();
            let Some(command) = Command::from_vec(commands) else {
                return "skip".into();
            };
            ("batch insert", command)
        }
        22 => {
            let (Some(scheme), Some(item)) = (scheme, item) else {
                return "skip".into();
            };
            let Some(existing) = workspace
                .schemes
                .get(&scheme)
                .and_then(|scheme| scheme.items.iter().find(|line| line.id == item))
            else {
                return "skip".into();
            };
            let mut replacement = existing.clone();
            replacement.content = knotq_model::ItemContent::text(name(rng, "replaced"));
            replacement.priority = (rng.below(4) as u8).checked_sub(1);
            (
                "replace item",
                Command::ReplaceItem {
                    scheme,
                    item: replacement,
                },
            )
        }
        23 => {
            // A cross-scheme move: a delete in one document and a fresh copy in
            // another, which is also how Daily Queue carry-over is expressed.
            // It is the command shape the whole `moved_edits` journal exists
            // for, so the law has to cover it.
            let (Some(source), Some(item)) = (scheme, item) else {
                return "skip".into();
            };
            let destinations: Vec<_> = schemes.iter().copied().filter(|id| *id != source).collect();
            let Some(destination) = rng.pick(&destinations) else {
                return "skip".into();
            };
            let Some(moved) = workspace
                .schemes
                .get(&source)
                .and_then(|scheme| scheme.items.iter().find(|line| line.id == item))
                .cloned()
            else {
                return "skip".into();
            };
            let Some(command) = Command::from_vec(vec![
                Command::DeleteItem {
                    scheme: source,
                    item,
                },
                Command::InsertItem {
                    scheme: destination,
                    position: 0,
                    item: moved,
                },
            ]) else {
                return "skip".into();
            };
            ("move item across schemes", command)
        }
        _ => {
            let Some(id) = scheme else {
                return "skip".into();
            };
            (
                "set gsync",
                Command::SetSchemeGsync {
                    id,
                    on: rng.below(2) == 0,
                },
            )
        }
    };

    let detail = format!("{label} {command:?}");
    match store.apply_local(command, CommandOrigin::User) {
        Ok(_) => detail,
        // A command the model rejects (moving a folder under itself, an index
        // that moved since the pick above) is not a projection failure — the
        // store is unchanged. Everything the model accepts must satisfy the law.
        Err(_) => format!("{label} (rejected)"),
    }
}

fn is_ancestor(workspace: &Workspace, ancestor: FolderId, of: FolderId) -> bool {
    let mut current = Some(of);
    while let Some(id) = current {
        if id == ancestor {
            return true;
        }
        current = workspace.folders.get(&id).and_then(|folder| folder.parent);
    }
    false
}

/// Where the store disagrees with its own documents, via the shared
/// [`knotq_sync::projection`] law (one implementation, used by this harness and
/// by the production-path fuzzer alike).
///
/// The documents are rebuilt from `document_states()` rather than read live, so
/// a divergence that only exists in the *persisted* encoding — the form a sync
/// pushes and a relaunch reloads — fails here too.
fn projection(store: &mut WorkspaceStore) -> Vec<String> {
    let states = store.crdt_document_states();
    let docs = WorkspaceCrdtDocuments::from_states(
        store.workspace(),
        knotq_model::ReplicaId::new(),
        &states,
    )
    .expect("rebuild documents from their persisted states");
    knotq_sync::projection::divergences(store.workspace(), &docs)
        .expect("materialize the workspace from its documents")
        .lines
}

fn run_seed(seed: u64, steps: usize) {
    knotq_model::set_deterministic_id_seed(Some(seed));
    let trace = std::env::var("KNOTQ_PROJECTION_TRACE").is_ok();
    let workspace = seed_workspace();
    let mut store = WorkspaceStore::new::<Vec<u8>>(
        workspace,
        knotq_model::ReplicaId::new(),
        false,
        HashMap::new(),
        1,
    );
    // A fresh store has not populated its documents yet; the first flush does.
    // Check from a populated baseline so a failure names the command that broke
    // the law rather than the construction that never established it.
    store.flush_crdt();
    let mut rng = Rng::new(seed);
    for step in 0..steps {
        let label = random_command(&mut store, &mut rng);
        if trace {
            eprintln!("[seed {seed} step {step:>4}] {label}");
        }
        let lines = projection(&mut store);
        assert!(
            lines.is_empty(),
            "seed {seed} step {step} ({label}): the workspace diverged from its own CRDT \
             documents — a later sync would materialize the CRDT's values and look like a \
             remote change nobody made:\n  {}",
            lines.join("\n  ")
        );
    }
}

#[test]
fn a_workspace_always_equals_what_its_own_crdt_documents_hold() {
    let seeds = env_usize("KNOTQ_PROJECTION_SEEDS", 24);
    let steps = env_usize("KNOTQ_PROJECTION_STEPS", 60);
    for seed in 0..seeds as u64 {
        run_seed(seed, steps);
    }
}

/// The same law after a relaunch: reconstructing the store from the states it
/// persisted must not change what the user sees.
#[test]
fn a_reloaded_workspace_still_equals_its_own_crdt_documents() {
    knotq_model::set_deterministic_id_seed(Some(7_001));
    let mut store = WorkspaceStore::new::<Vec<u8>>(
        seed_workspace(),
        knotq_model::ReplicaId::new(),
        false,
        HashMap::new(),
        1,
    );
    store.flush_crdt();
    let mut rng = Rng::new(7_001);
    for _ in 0..80 {
        random_command(&mut store, &mut rng);
    }
    let states: HashMap<_, _> = store
        .crdt_document_states()
        .into_iter()
        .map(|(id, bytes)| (id, bytes.to_vec()))
        .collect();
    let saved = store.workspace().clone();

    let mut reloaded = WorkspaceStore::new(
        saved.clone(),
        knotq_model::ReplicaId::new(),
        false,
        states,
        1,
    );
    reloaded.flush_crdt();
    let lines = projection(&mut reloaded);
    assert!(
        lines.is_empty(),
        "a relaunched store diverged from the states it persisted:\n  {}",
        lines.join("\n  ")
    );
}
