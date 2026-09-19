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
use knotq_commands::Command;
use knotq_model::{daily_queue_displaced_item_id, FolderId, ItemId, NodeRef, SchemeId, Workspace};

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

    /// What content EXISTS, independent of any field's value: the identity of
    /// every item, scheme, folder and Daily binding the user can reach. A field
    /// that legitimately resolved to another device's value is not missing
    /// content, so a loss check compares these rather than whole lines.
    pub(super) fn content_keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = Vec::new();
        keys.extend(self.items.keys().map(|item| format!("item {item}")));
        keys.extend(self.schemes.keys().map(|scheme| format!("scheme {scheme}")));
        keys.extend(
            self.folder_parent
                .keys()
                .map(|folder| format!("folder {folder}")),
        );
        keys.extend(self.daily.keys().map(|date| format!("daily {date}")));
        keys.sort();
        keys
    }

    pub(super) fn newly_visible_folders(&self, before: &View) -> Vec<FolderId> {
        self.folder_parent
            .keys()
            .filter(|folder| !before.folder_parent.contains_key(folder))
            .copied()
            .collect()
    }

    pub(super) fn newly_visible_schemes(&self, before: &View) -> Vec<SchemeId> {
        self.scheme_parent
            .keys()
            .filter(|scheme| !before.scheme_parent.contains_key(scheme))
            .copied()
            .collect()
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
    /// Where an item was moved to, for items whose scheme a local step changed.
    /// See [`Attribution::moved_into_a_destroyed_scheme`].
    moved_into: HashMap<ItemId, SchemeId>,
    /// A newly-created node can be temporarily re-homed to root when its
    /// requested parent is absent from a stale replica. When the parent arrives
    /// through sync, the node returns to the parent the local create intended;
    /// that is not an unexplained remote move.
    created_folder_parent: HashMap<FolderId, FolderId>,
    created_scheme_parent: HashMap<SchemeId, FolderId>,
    writers: HashMap<Key, HashSet<usize>>,
    /// Non-seed item writes can make a stale duplicate become the visible
    /// deterministic winner without any placement command.
    item_writers: HashMap<ItemId, HashSet<usize>>,
    /// Placements observed in each device's local view. These are evidence for
    /// a duplicate-winner switch during a later sync.
    observed_item_placements: HashMap<(usize, ItemId), SchemeId>,
    /// A displaced Daily Queue row is a derived archive copy of the live row
    /// that was carried forward. Its metadata can legitimately converge from
    /// a stale copy to the live row's winning value, even though no command
    /// directly edited the derived id.
    archive_sources: HashMap<ItemId, ItemId>,
}

impl Attribution {
    fn record_archive_relationships(&mut self, view: &View) {
        for source in view.items.keys() {
            for (date, scheme) in &view.daily {
                let displaced = daily_queue_displaced_item_id(*source, *date);
                if view.items.get(&displaced) == Some(scheme) {
                    self.archive_sources.insert(displaced, *source);
                }
            }
        }
    }

    /// Record a local step on `device`.
    pub(super) fn record_local(&mut self, device: usize, before: &View, after: &View) {
        self.record_archive_relationships(before);
        self.record_archive_relationships(after);
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
        for (folder, parent) in &after.folder_parent {
            if !before.folder_parent.contains_key(folder) {
                if let Some(parent) = parent {
                    self.created_folder_parent.insert(*folder, *parent);
                }
            }
        }
        for (scheme, parent) in &after.scheme_parent {
            if !before.scheme_parent.contains_key(scheme) {
                if let Some(parent) = parent {
                    self.created_scheme_parent.insert(*scheme, *parent);
                }
            }
        }
        // An item whose scheme changed in this step was MOVED. Remember where it
        // went: if that destination is destroyed by any device, the item going
        // with it is explained rather than lost. Recorded for every move, and
        // consulted only when the destination turns out to be destroyed.
        for (item, scheme) in &after.items {
            if before.items.get(item).is_some_and(|from| from != scheme) {
                self.moved_into.insert(*item, *scheme);
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
                if let Subject::Item(item) = key.0 {
                    self.item_writers.entry(item).or_default().insert(device);
                }
            }
        }
        for (item, scheme) in &after.items {
            self.observed_item_placements
                .insert((device, *item), *scheme);
        }
    }

    /// Record fields explicitly targeted by accepted local commands, including
    /// commands that are idempotent on the issuing device's stale view.
    ///
    /// A before/after diff cannot see `SetFolderExpanded { expanded: true }`
    /// when the local copy already says `true`, but that command still creates
    /// a legitimate CRDT write. Without this intent record, a later merge can
    /// move the value on another device and the oracle would call the change a
    /// silent loss even though another device did issue the write.
    pub(super) fn record_command_intent<'a>(
        &mut self,
        device: usize,
        commands: impl IntoIterator<Item = &'a Command>,
    ) {
        for command in commands {
            self.record_one_command_intent(device, command);
        }
    }

    pub(super) fn record_creation_intent<'a>(
        &mut self,
        folders: impl IntoIterator<Item = FolderId>,
        schemes: impl IntoIterator<Item = SchemeId>,
        commands: impl IntoIterator<Item = &'a Command>,
    ) {
        let mut folders = folders.into_iter();
        let mut schemes = schemes.into_iter();
        for command in commands {
            self.record_one_creation_intent(command, &mut folders, &mut schemes);
        }
    }

    fn record_one_creation_intent(
        &mut self,
        command: &Command,
        folders: &mut impl Iterator<Item = FolderId>,
        schemes: &mut impl Iterator<Item = SchemeId>,
    ) {
        match command {
            Command::CreateFolder { parent, .. } => {
                if let Some(folder) = folders.next() {
                    self.created_folder_parent.insert(folder, *parent);
                }
            }
            Command::CreateScheme { folder, .. } => {
                if let Some(scheme) = schemes.next() {
                    self.created_scheme_parent.insert(scheme, *folder);
                }
            }
            Command::Batch(commands) => {
                for command in commands {
                    self.record_one_creation_intent(command, folders, schemes);
                }
            }
            _ => {}
        }
    }

    fn record_one_command_intent(&mut self, device: usize, command: &Command) {
        let mut write = |subject: Subject, field: Field| {
            self.writers
                .entry((subject, field))
                .or_default()
                .insert(device);
            if let Subject::Item(item) = subject {
                self.item_writers.entry(item).or_default().insert(device);
            }
        };
        match command {
            Command::Batch(commands) => {
                for command in commands {
                    self.record_one_command_intent(device, command);
                }
            }
            Command::CreateFolder { .. } => {}
            Command::RestoreFolder {
                parent: _parent,
                folder,
                ..
            } => {
                // Restoring a folder can be idempotent in the issuing
                // device's view, but it still rewrites the folder snapshot
                // into the workspace CRDT. Attribute every visible field it
                // carries so a later merge is not mistaken for a silent
                // expansion/name/parent/archive loss.
                write(Subject::Folder(folder.id), Field::FolderName);
                write(Subject::Folder(folder.id), Field::FolderParent);
                write(Subject::Folder(folder.id), Field::FolderExpanded);
                write(Subject::Folder(folder.id), Field::FolderArchived);
            }
            Command::RestoreDeletedFolder {
                folders, schemes, ..
            } => {
                for folder in folders {
                    write(Subject::Folder(folder.id), Field::FolderName);
                    write(Subject::Folder(folder.id), Field::FolderParent);
                    write(Subject::Folder(folder.id), Field::FolderExpanded);
                    write(Subject::Folder(folder.id), Field::FolderArchived);
                }
                for scheme in schemes {
                    write(Subject::Scheme(scheme.id), Field::SchemeName);
                    write(Subject::Scheme(scheme.id), Field::SchemeParent);
                    write(Subject::Scheme(scheme.id), Field::SchemeColor);
                    write(Subject::Scheme(scheme.id), Field::SchemeSource);
                    write(Subject::Scheme(scheme.id), Field::SchemeArchived);
                    for item in &scheme.items {
                        write(Subject::Item(item.id), Field::ItemContent);
                        write(Subject::Item(item.id), Field::ItemMeta);
                        write(Subject::Item(item.id), Field::ItemIndent);
                        write(Subject::Item(item.id), Field::ItemPlacement);
                    }
                }
            }
            Command::RenameFolder { id, .. } => write(Subject::Folder(*id), Field::FolderName),
            Command::SetFolderExpanded { id, .. } => {
                write(Subject::Folder(*id), Field::FolderExpanded)
            }
            Command::DeleteFolder { id } | Command::PermanentlyDeleteFolder { id } => {
                write(Subject::Folder(*id), Field::FolderArchived)
            }
            Command::CreateScheme { .. }
            | Command::RestoreScheme { .. }
            | Command::RestoreDeletedScheme { .. } => {}
            Command::RenameScheme { id, .. } => write(Subject::Scheme(*id), Field::SchemeName),
            Command::SetSchemeColor { id, .. } => write(Subject::Scheme(*id), Field::SchemeColor),
            Command::SetSchemeGsync { id, .. } | Command::SetSchemeSource { id, .. } => {
                write(Subject::Scheme(*id), Field::SchemeSource)
            }
            Command::DeleteScheme { id } | Command::PermanentlyDeleteScheme { id } => {
                write(Subject::Scheme(*id), Field::SchemeArchived)
            }
            Command::MoveNode { node, .. } => match node {
                NodeRef::Folder(id) => write(Subject::Folder(*id), Field::FolderParent),
                NodeRef::Scheme(id) => write(Subject::Scheme(*id), Field::SchemeParent),
            },
            Command::EnsureDailyQueue { .. } => {}
            Command::InsertItem { item, .. } => {
                write(Subject::Item(item.id), Field::ItemContent);
                write(Subject::Item(item.id), Field::ItemMeta);
                write(Subject::Item(item.id), Field::ItemIndent);
                write(Subject::Item(item.id), Field::ItemPlacement);
            }
            Command::UpdateItemText { item, .. } => write(Subject::Item(*item), Field::ItemContent),
            Command::ReplaceItem { item, .. } => {
                write(Subject::Item(item.id), Field::ItemContent);
                write(Subject::Item(item.id), Field::ItemMeta);
                write(Subject::Item(item.id), Field::ItemIndent);
                write(Subject::Item(item.id), Field::ItemPlacement);
            }
            Command::SetItemIndent { item, .. } => write(Subject::Item(*item), Field::ItemIndent),
            Command::SetItemMarker { item, .. }
            | Command::SetItemMarkerFamily { item, .. }
            | Command::SetItemDate { item, .. }
            | Command::SetItemRecurrence { item, .. }
            | Command::SetItemPriority { item, .. }
            | Command::SetOccurrenceNotificationOffset { item, .. }
            | Command::ToggleOccurrence { item, .. } => {
                write(Subject::Item(*item), Field::ItemMeta)
            }
            Command::DeleteItem { item, .. } => write(Subject::Item(*item), Field::ItemPlacement),
            Command::ReorderItem { .. } => {}
        }
    }

    /// Record a starting state every device begins with (the seeded starter
    /// workspace): all of its fields count as written by `device`.
    pub(super) fn record_seed(&mut self, device: usize, view: &View) {
        self.record_archive_relationships(view);
        for key in view.fields.keys() {
            self.writers.entry(*key).or_default().insert(device);
        }
        for (item, scheme) in &view.items {
            self.observed_item_placements
                .insert((device, *item), *scheme);
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

    /// An item moved into a scheme that some device then destroyed goes with
    /// that scheme; its disappearance is explained, not a silent loss.
    ///
    /// The two steps are concurrent by construction: the moving device had not
    /// yet pulled the destroy, so its own workspace still held the scheme,
    /// `editable_schemes` still offered it as a target, and `insert_item`
    /// legitimately applied. When the destroy wins the merge, the line has no
    /// home to land in. (Production fuzz seed 10001: device 0 last synced at
    /// step 51, device 1 permanently deleted the scheme at step 61, device 0
    /// moved a line into it at step 92.)
    ///
    /// Deliberately narrow: it excuses ONLY an item whose recorded move
    /// destination is itself destroyed. An item that vanishes from a live
    /// scheme, or one that was never moved, is still a violation.
    fn moved_into_a_destroyed_scheme(&self, item: ItemId) -> bool {
        self.moved_into
            .get(&item)
            .is_some_and(|destination| self.destroyed_schemes.contains(destination))
    }

    fn moved_by_another_device(&self, device: usize, item: ItemId) -> bool {
        // `usize::MAX` is the synthetic server actor used by `audit_server`.
        // A server-side disappearance is never explained by a placement write:
        // the exception only covers a device seeing an item move to another
        // scheme during a concurrent merge.
        if device == usize::MAX {
            return false;
        }
        self.writers
            .get(&(Subject::Item(item), Field::ItemPlacement))
            .is_some_and(|writers| writers.iter().any(|writer| *writer != device))
    }

    /// A stale duplicate can become the visible winner when another device
    /// edits that copy. The item appears to move during sync even though no
    /// placement field was written in that step; the remote metadata write and
    /// the other device's observed placement are the proof that this is the
    /// deterministic cross-document dedupe transition, not silent movement.
    fn duplicate_winner_switch(&self, device: usize, item: ItemId, new_placement: &str) -> bool {
        let Some(writers) = self.item_writers.get(&item) else {
            return false;
        };
        writers.iter().any(|writer| {
            *writer != device
                && self
                    .observed_item_placements
                    .get(&(*writer, item))
                    .is_some_and(|scheme| scheme.to_string() == new_placement)
        })
    }

    /// A stale duplicate can briefly make a previously acknowledged move look
    /// like it was undone during materialization. The landing then reasserts
    /// the device's own last observed placement. This is not a new remote
    /// write, but it is also not a loss: the value is returning to the exact
    /// placement that this device already authored and still observes as its
    /// intended result.
    fn local_placement_reassertion(
        &self,
        device: usize,
        item: ItemId,
        new_placement: &str,
    ) -> bool {
        if device == usize::MAX {
            return false;
        }
        self.writers
            .get(&(Subject::Item(item), Field::ItemPlacement))
            .is_some_and(|writers| writers.contains(&device))
            && self
                .observed_item_placements
                .get(&(device, item))
                .is_some_and(|scheme| scheme.to_string() == new_placement)
    }

    fn derived_archive_field_explained(&self, device: usize, key: &Key) -> bool {
        let (Subject::Item(item), field) = *key else {
            return false;
        };
        if !matches!(
            field,
            Field::ItemContent | Field::ItemMeta | Field::ItemIndent
        ) {
            return false;
        }
        let Some(source) = self.archive_sources.get(&item) else {
            return false;
        };
        self.writers
            .get(&(Subject::Item(*source), field))
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
            // A permanently deleted folder is no longer present in `view`,
            // but a scheme concurrently created under it still gets the
            // folder's archive semantics during index materialization. The
            // explicit archive writer is the proof this is derived state,
            // rather than an unexplained missing parent.
            if self.writers.contains_key(&archive)
                && (view.fields.get(&archive).map(String::as_str) == Some("true")
                    || self.destroyed_folders.contains(&folder))
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

    /// A node whose parent folder some device DESTROYED has to go somewhere,
    /// and the root is the only safe home — the index materializer re-homes a
    /// node whose parent is missing rather than dropping it.
    ///
    /// The disappearance check already excuses a scheme that vanished *under* a
    /// destroyed folder (`folder_destroyed_or_under_destroyed`). Not excusing
    /// the SURVIVING node's re-homing means the oracle reports the non-lossy
    /// outcome while accepting the lossy one, which is backwards.
    ///
    /// Deliberately narrow: the old parent must be a folder some device really
    /// destroyed, and the new parent must be the root (or none). A move between
    /// two folders that both still exist is still reported, so this cannot hide
    /// a node being silently re-homed while its parent is alive — which is the
    /// symptom that travels with a genuine folder LOSS (production fuzz seed
    /// 10034: device 1 empties the trash at step 113, device 2 creates a scheme
    /// into that same folder at 115 having not seen it, and device 0 pulls both
    /// at 195).
    fn reparented_by_a_destroyed_folder(&self, key: &Key, old: &str, new: &str) -> bool {
        if !matches!(
            key,
            (Subject::Scheme(_), Field::SchemeParent) | (Subject::Folder(_), Field::FolderParent)
        ) {
            return false;
        }
        if new != "root" && new != "None" {
            return false;
        }
        if let Subject::Scheme(scheme) = key.0 {
            // A scheme created under a folder can survive as an archived node
            // when another device permanently deletes that folder from a stale
            // view. Its parent becoming None is derived tombstone behavior, not
            // an unexplained move. The creation-parent record is the only
            // evidence needed, and keeps this exception narrower than excusing
            // every root/None transition.
            if new == "None"
                && self
                    .created_scheme_parent
                    .get(&scheme)
                    .is_some_and(|folder| self.destroyed_folders.contains(folder))
            {
                return true;
            }
        }
        self.destroyed_folders
            .iter()
            .any(|folder| *old == format!("{:?}", Some(*folder)))
    }

    /// A scheme created under a folder can be restored under a different
    /// folder while another replica still carries the original parent. When
    /// that stale replica later observes the original folder being archived,
    /// workspace-index materialization may detach the scheme (`parent = None`)
    /// even though no device issued a move to `None`.
    ///
    /// This is deliberately limited to the scheme's recorded creation parent,
    /// an explicit archive writer for that exact folder, and the derived
    /// `None` destination. A normal move between live folders is not excused.
    fn reparented_by_an_archived_origin(&self, device: usize, key: &Key, new: &str) -> bool {
        let (Subject::Scheme(scheme), Field::SchemeParent) = *key else {
            return false;
        };
        if new != "None" {
            return false;
        }
        let Some(origin) = self.created_scheme_parent.get(&scheme) else {
            return false;
        };
        self.writers
            .get(&(Subject::Folder(*origin), Field::FolderArchived))
            .is_some_and(|writers| writers.iter().any(|writer| *writer != device))
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
                || self.moved_into_a_destroyed_scheme(*item)
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
                // The root folder is recorded as the label "root" rather than by
                // id, because its id is re-derived from the account on first
                // sign-in. A folder or scheme parented to the root therefore
                // LOOKS like it changed parent the moment the device adopts a
                // different root id: the parent field still names the old root,
                // which is no longer "the root". Nothing moved, and no device
                // wrote anything. A genuine move names a folder that is neither
                // view's root, so it is still reported.
                let parent_field = matches!(
                    key,
                    (Subject::Folder(_), Field::FolderParent)
                        | (Subject::Scheme(_), Field::SchemeParent)
                );
                let parent_unchanged_across_root_change = parent_field
                    && ((old.as_str() == "root"
                        && before
                            .root
                            .is_some_and(|root| *new == format!("{:?}", Some(root))))
                        || (new.as_str() == "root"
                            && after
                                .root
                                .is_some_and(|root| *old == format!("{:?}", Some(root)))));
                // Archived schemes are intentionally removed from the active
                // folder tree during workspace-index materialization. A stale
                // in-memory copy can still show the old parent immediately
                // before that normalization; the archive bit staying true
                // proves this is index cleanup, not a user move or data loss.
                let archived_scheme_detach = match key {
                    (Subject::Scheme(scheme), Field::SchemeParent) => {
                        let archive_key = (Subject::Scheme(*scheme), Field::SchemeArchived);
                        before.fields.get(&archive_key).map(String::as_str) == Some("true")
                            && after.fields.get(&archive_key).map(String::as_str) == Some("true")
                    }
                    _ => false,
                };
                let locally_created_parent_restored = match key {
                    (Subject::Folder(folder), Field::FolderParent) => self
                        .created_folder_parent
                        .get(folder)
                        .is_some_and(|parent| *new == format!("{:?}", Some(*parent))),
                    (Subject::Scheme(scheme), Field::SchemeParent) => self
                        .created_scheme_parent
                        .get(scheme)
                        .is_some_and(|parent| *new == format!("{:?}", Some(*parent))),
                    _ => false,
                };
                // Permanent deletion of a folder also removes the schemes it
                // contained, even when this replica's stale view had that
                // scheme under a different parent. The tombstoned scheme can
                // therefore surface as `parent = None` during index merge;
                // the local disappearance record is the causal writer for
                // that derived parent transition.
                let destroyed_node_detach = match key {
                    (Subject::Scheme(scheme), Field::SchemeParent) => {
                        self.destroyed_schemes.contains(scheme)
                    }
                    (Subject::Folder(folder), Field::FolderParent) => {
                        self.destroyed_folders.contains(folder)
                    }
                    _ => false,
                };
                let duplicate_winner_switch = match key {
                    (Subject::Item(item), Field::ItemPlacement) => {
                        self.duplicate_winner_switch(device, *item, new)
                    }
                    _ => false,
                };
                let local_placement_reassertion = match key {
                    (Subject::Item(item), Field::ItemPlacement) => {
                        self.local_placement_reassertion(device, *item, new)
                    }
                    _ => false,
                };
                let explained = self
                    .writers
                    .get(key)
                    .is_some_and(|writers| writers.iter().any(|writer| *writer != device))
                    || self.archived_with_its_folder(key, new, after)
                    || self.archived_with_its_folder(key, new, before)
                    || parent_unchanged_across_root_change
                    || archived_scheme_detach
                    || locally_created_parent_restored
                    || destroyed_node_detach
                    || self.reparented_by_a_destroyed_folder(key, old, new)
                    || self.reparented_by_an_archived_origin(device, key, new)
                    || duplicate_winner_switch
                    || local_placement_reassertion
                    || self.derived_archive_field_explained(device, key);
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
