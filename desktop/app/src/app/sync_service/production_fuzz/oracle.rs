//! The no-silent-loss oracle.
//!
//! Convergence alone cannot see a loss every device agrees on: if a sync drops
//! a scheme everywhere, the devices, the server and a freshly signed-in device
//! all still match. So every change is attributed.
//!
//! A *local* step (a command, undo, carryover, a day coming into being, …) on a
//! device is diffed before/after and recorded: what it destroyed, and which
//! user-visible fields it wrote. A *passive* step (a sync run, a relaunch)
//! must then be explained by those records:
//!
//!  - nothing may disappear that no device destroyed, and
//!  - a field may only change if some *other* device wrote it — a field only
//!    this device (or nobody) ever wrote that moves during its own sync is a
//!    reverted or corrupted edit.
//!
//! Nothing here interprets commands, so every local path — including undo,
//! which applies an inverse the fuzzer never sees — is covered the same way.

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::NaiveDate;
use knotq_model::{FolderId, ItemId, NodeRef, SchemeId, Workspace};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(super) enum Subject {
    Item(ItemId),
    Scheme(SchemeId),
    Folder(FolderId),
}

/// User-visible attributes, grouped so fields the model couples (a checkbox
/// marker and the dates/completion that require it) share one writer set.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(super) enum Field {
    ItemContent,
    ItemMeta,
    ItemIndent,
    ItemPlacement,
    SchemeName,
    SchemeColor,
    SchemeArchived,
    SchemeParent,
    SchemeSource,
    FolderName,
    FolderParent,
    FolderArchived,
    FolderExpanded,
}

type Key = (Subject, Field);

/// What a user can see of a workspace, flattened into attributable fields.
#[derive(Clone, Debug, Default)]
pub(super) struct View {
    fields: HashMap<Key, String>,
    /// Which scheme each item is in.
    items: HashMap<ItemId, SchemeId>,
    schemes: HashMap<SchemeId, String>,
    scheme_parent: HashMap<SchemeId, Option<FolderId>>,
    folder_parent: HashMap<FolderId, Option<FolderId>>,
    /// The workspace's root folder: never user-visible, never losable.
    root: Option<FolderId>,
    daily: BTreeMap<NaiveDate, SchemeId>,
    /// Item ids that occur more than once — never legitimate.
    duplicate_items: Vec<ItemId>,
    /// Order-sensitive lines used only for the convergence comparison.
    order: Vec<String>,
}

impl View {
    pub(super) fn of(workspace: &Workspace) -> Self {
        let mut view = View::default();
        // The root folder is not user-visible, and its id is re-derived from the
        // account on first sign-in — so it is recorded as "root", never as a
        // folder that can be lost or a parent that can change.
        let root = workspace.root;
        view.root = Some(root);
        let label = |folder: Option<FolderId>| match folder {
            Some(id) if id == root => "root".to_string(),
            other => format!("{other:?}"),
        };
        let mut parent_of: HashMap<NodeRef, FolderId> = HashMap::new();
        for (id, folder) in &workspace.folders {
            for child in &folder.children {
                parent_of.insert(*child, *id);
            }
        }
        view.order
            .push(format!("trash {:?}", workspace.recently_deleted));
        view.order.push(format!(
            "trash-folders {:?}",
            workspace.recently_deleted_folders
        ));

        // Only what the user can reach counts: folders reachable from the root
        // or sitting in the trash (with their subtrees), and the schemes in
        // them, in the trash, or bound as a Daily Queue day. A folder or scheme
        // that falls out of that set is gone as far as the user can tell, even if
        // it lingers in the maps; one that never entered it (another identity's
        // pre-sign-in root riding along in an index) was never visible.
        let mut visible_folders: HashSet<FolderId> = HashSet::new();
        let mut stack: Vec<FolderId> = vec![root];
        stack.extend(workspace.recently_deleted_folders.iter().copied());
        while let Some(folder) = stack.pop() {
            if !visible_folders.insert(folder) {
                continue;
            }
            if let Some(entry) = workspace.folders.get(&folder) {
                for child in &entry.children {
                    if let NodeRef::Folder(child) = child {
                        stack.push(*child);
                    }
                }
            }
        }
        for folder in &visible_folders {
            if let Some(entry) = workspace.folders.get(folder) {
                view.order.push(format!(
                    "folder {} children {:?}",
                    label(Some(*folder)),
                    entry.children
                ));
            }
        }
        let mut visible_schemes: HashSet<SchemeId> = workspace
            .recently_deleted
            .iter()
            .chain(workspace.daily_queue.values())
            .copied()
            .collect();
        for folder in &visible_folders {
            if let Some(entry) = workspace.folders.get(folder) {
                for child in &entry.children {
                    if let NodeRef::Scheme(scheme) = child {
                        visible_schemes.insert(*scheme);
                    }
                }
            }
        }

        let mut seen_items = HashSet::new();
        for (id, scheme) in &workspace.schemes {
            if !visible_schemes.contains(id) {
                continue;
            }
            let subject = Subject::Scheme(*id);
            view.schemes.insert(*id, scheme.name.clone());
            let parent = parent_of.get(&NodeRef::Scheme(*id)).copied();
            view.scheme_parent
                .insert(*id, parent.filter(|parent| *parent != root));
            view.fields
                .insert((subject, Field::SchemeName), scheme.name.clone());
            view.fields.insert(
                (subject, Field::SchemeColor),
                scheme.color_index.to_string(),
            );
            view.fields.insert(
                (subject, Field::SchemeArchived),
                workspace.is_scheme_deleted(*id).to_string(),
            );
            view.fields
                .insert((subject, Field::SchemeParent), label(parent));
            view.fields.insert(
                (subject, Field::SchemeSource),
                serde_json::to_string(&(&scheme.source, scheme.gsync)).unwrap_or_default(),
            );
            view.order.push(format!(
                "scheme {id} items {:?}",
                scheme.items.iter().map(|item| item.id).collect::<Vec<_>>()
            ));
            for item in &scheme.items {
                if !seen_items.insert(item.id) {
                    view.duplicate_items.push(item.id);
                }
                let subject = Subject::Item(item.id);
                view.items.insert(item.id, *id);
                let mut normalized = item.clone();
                normalized.normalize_state();
                view.fields.insert(
                    (subject, Field::ItemContent),
                    serde_json::to_string(&normalized.content).unwrap_or_default(),
                );
                view.fields.insert(
                    (subject, Field::ItemMeta),
                    serde_json::to_string(&(
                        normalized.marker,
                        normalized.marker_family,
                        normalized.start,
                        normalized.end,
                        normalized.available,
                        &normalized.repeats,
                        &normalized.state,
                        normalized.priority,
                        &normalized.external,
                    ))
                    .unwrap_or_default(),
                );
                view.fields
                    .insert((subject, Field::ItemIndent), item.indent.to_string());
                view.fields
                    .insert((subject, Field::ItemPlacement), id.to_string());
            }
        }

        for (id, folder) in &workspace.folders {
            if *id == root || !visible_folders.contains(id) {
                continue;
            }
            let subject = Subject::Folder(*id);
            view.folder_parent
                .insert(*id, folder.parent.filter(|parent| *parent != root));
            view.fields
                .insert((subject, Field::FolderName), folder.name.clone());
            view.fields
                .insert((subject, Field::FolderParent), label(folder.parent));
            view.fields.insert(
                (subject, Field::FolderArchived),
                workspace.is_folder_deleted(*id).to_string(),
            );
            view.fields.insert(
                (subject, Field::FolderExpanded),
                folder.expanded.to_string(),
            );
        }
        view.daily = workspace.daily_queue.clone();
        view
    }

    /// Every line a user could see, sorted — two devices converged iff equal.
    pub(super) fn convergence_lines(&self) -> Vec<String> {
        let mut lines: Vec<String> = self
            .fields
            .iter()
            .map(|((subject, field), value)| format!("{subject:?} {field:?} = {value}"))
            .collect();
        lines.extend(self.order.iter().cloned());
        lines.extend(
            self.daily
                .iter()
                .map(|(date, scheme)| format!("daily {date} -> {scheme}")),
        );
        lines.sort();
        lines
    }
}

/// Who changed what, across every device and account.
#[derive(Default)]
pub(super) struct Attribution {
    destroyed_items: HashSet<ItemId>,
    destroyed_schemes: HashSet<SchemeId>,
    destroyed_folders: HashSet<FolderId>,
    writers: HashMap<Key, HashSet<usize>>,
}

impl Attribution {
    /// Record a local step on `device`.
    pub(super) fn record_local(&mut self, device: usize, before: &View, after: &View) {
        for scheme in before.schemes.keys() {
            if !after.schemes.contains_key(scheme) {
                self.destroyed_schemes.insert(*scheme);
            }
        }
        for item in before.items.keys() {
            if !after.items.contains_key(item) {
                self.destroyed_items.insert(*item);
            }
        }
        for folder in before.folder_parent.keys() {
            if !after.folder_parent.contains_key(folder) {
                self.destroyed_folders.insert(*folder);
            }
        }
        for (key, value) in &after.fields {
            if before.fields.get(key) != Some(value) {
                self.writers.entry(*key).or_default().insert(device);
            }
        }
    }

    /// Record a starting state every device begins with (the seeded starter
    /// workspace): all of its fields count as written by `device`.
    pub(super) fn record_seed(&mut self, device: usize, view: &View) {
        for key in view.fields.keys() {
            self.writers.entry(*key).or_default().insert(device);
        }
    }

    fn folder_destroyed_or_under_destroyed(&self, view: &View, folder: Option<FolderId>) -> bool {
        let mut current = folder;
        let mut hops = 0;
        while let Some(id) = current {
            if self.destroyed_folders.contains(&id) {
                return true;
            }
            hops += 1;
            if hops > 64 {
                return false;
            }
            current = view.folder_parent.get(&id).copied().flatten();
        }
        false
    }

    fn scheme_excused(&self, view: &View, scheme: SchemeId) -> bool {
        self.destroyed_schemes.contains(&scheme)
            || self.folder_destroyed_or_under_destroyed(
                view,
                view.scheme_parent.get(&scheme).copied().flatten(),
            )
    }

    fn moved_by_another_device(&self, device: usize, item: ItemId) -> bool {
        self.writers
            .get(&(Subject::Item(item), Field::ItemPlacement))
            .is_some_and(|writers| writers.iter().any(|writer| *writer != device))
    }

    /// A scheme inside a folder some device archived is archived with it, even
    /// when that device never saw the scheme there: another device moved it in
    /// concurrently and the merge marks it. No device's own step changed the
    /// scheme's flag, but the folder's archive explains it.
    fn archived_with_its_folder(&self, key: &Key, new: &str, view: &View) -> bool {
        let (Subject::Scheme(scheme), Field::SchemeArchived) = *key else {
            return false;
        };
        if new != "true" {
            return false;
        }
        let mut current = view.scheme_parent.get(&scheme).copied().flatten();
        let mut hops = 0;
        while let Some(folder) = current {
            let archive = (Subject::Folder(folder), Field::FolderArchived);
            if view.fields.get(&archive).map(String::as_str) == Some("true")
                && self.writers.contains_key(&archive)
            {
                return true;
            }
            hops += 1;
            if hops > 64 {
                return false;
            }
            current = view.folder_parent.get(&folder).copied().flatten();
        }
        false
    }

    /// Check a passive step (`label`) on `device`: every disappearance and
    /// every field change must be explained by some device's local step.
    /// `check_fields` is off for the final settle comparison, where fields
    /// legitimately move to other devices' values.
    pub(super) fn check_passive(
        &self,
        device: usize,
        label: &str,
        before: &View,
        after: &View,
        check_fields: bool,
    ) -> Vec<String> {
        let mut violations = Vec::new();
        for (scheme, name) in &before.schemes {
            if !after.schemes.contains_key(scheme) && !self.scheme_excused(before, *scheme) {
                violations.push(format!(
                    "device {device}: {label} lost scheme {scheme} {name:?} that no device deleted"
                ));
            }
        }
        for (item, scheme) in &before.items {
            if after.items.contains_key(item)
                || self.destroyed_items.contains(item)
                || self.scheme_excused(before, *scheme)
                || self.moved_by_another_device(device, *item)
            {
                continue;
            }
            violations.push(format!(
                "device {device}: {label} lost item {item} (in scheme {scheme} {:?}) that no device deleted",
                before.schemes.get(scheme)
            ));
        }
        for folder in before.folder_parent.keys() {
            // A folder that became the root (the account's canonical root,
            // adopted on first sign-in) is not a folder the user lost.
            if !after.folder_parent.contains_key(folder)
                && after.root != Some(*folder)
                && !self.folder_destroyed_or_under_destroyed(before, Some(*folder))
            {
                violations.push(format!(
                    "device {device}: {label} lost folder {folder} that no device deleted"
                ));
            }
        }
        for (date, scheme) in &before.daily {
            if !after.daily.contains_key(date) {
                violations.push(format!(
                    "device {device}: {label} lost the Daily Queue binding for {date} ({scheme})"
                ));
            }
        }
        for item in &after.duplicate_items {
            if before.duplicate_items.contains(item) {
                continue;
            }
            violations.push(format!(
                "device {device}: {label} left item {item} in the workspace more than once"
            ));
        }
        if check_fields {
            for (key, old) in &before.fields {
                let Some(new) = after.fields.get(key) else {
                    continue;
                };
                if new == old {
                    continue;
                }
                let explained = self
                    .writers
                    .get(key)
                    .is_some_and(|writers| writers.iter().any(|writer| *writer != device))
                    || self.archived_with_its_folder(key, new, after);
                if !explained {
                    violations.push(format!(
                        "device {device}: {label} changed {key:?} with no other device ever writing it: {old} -> {new}"
                    ));
                }
            }
        }
        violations
    }
}

/// Lines only in `left` and only in `right`, for a readable divergence report.
pub(super) fn diff_lines(left: &[String], right: &[String]) -> (Vec<String>, Vec<String>) {
    let left_set: HashSet<&String> = left.iter().collect();
    let right_set: HashSet<&String> = right.iter().collect();
    (
        left.iter()
            .filter(|line| !right_set.contains(line))
            .cloned()
            .collect(),
        right
            .iter()
            .filter(|line| !left_set.contains(line))
            .cloned()
            .collect(),
    )
}
