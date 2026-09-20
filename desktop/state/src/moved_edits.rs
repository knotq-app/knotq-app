//! Keeping local item edits when a sync landing materializes a competing copy.
//!
//! Each scheme is its own CRDT document, so moving a line between schemes is a
//! delete in the source and a fresh copy in the target — a copy of the line as
//! the moving device saw it (carry-over into today's page is exactly this). An
//! edit this device made to the source copy meanwhile lands on a deleted line
//! and is lost for every device. When a sync brings such a move in, this device
//! still knows which fields it edited, so it re-applies them to the moved line
//! as a new edit, which every device then converges on.

use std::collections::{HashMap, HashSet};

use knotq_commands::{Command, CommandOrigin, DateKind};
use knotq_model::{
    Item, ItemId, ItemMarker, MarkerFamily, OccurrenceState, OperationId, Scheme, SchemeId,
    Workspace,
};
use knotq_sync::{QueuedItemFields, RecentItemEdit};

use crate::AppState;

#[derive(Clone, Copy, Default)]
struct EditedFields {
    whole: bool,
    /// A field-level edit restored from the durable journal may still bridge
    /// a move that rematerialized into the same scheme.
    restart_bridge: bool,
    /// A restored journal entry must not replay on its original scheme, even
    /// when its fields are not safe for the default-field move bridge.
    restart_guard: bool,
    content: bool,
    marker: bool,
    marker_family: bool,
    indent: bool,
    start: bool,
    end: bool,
    available: bool,
    recurrence: bool,
    priority: bool,
    state: bool,
}

impl EditedFields {
    /// The persisted form (see `LocalSyncState::queued_item_fields`). Bit
    /// positions are stored on disk: append only.
    fn bits(&self) -> u32 {
        [
            self.whole,
            self.content,
            self.marker,
            self.marker_family,
            self.indent,
            self.start,
            self.end,
            self.available,
            self.recurrence,
            self.priority,
            self.state,
        ]
        .iter()
        .enumerate()
        .fold(
            0,
            |bits, (bit, set)| if *set { bits | (1 << bit) } else { bits },
        )
    }

    fn from_bits(bits: u32) -> Self {
        let set = |bit: u32| bits & (1 << bit) != 0;
        Self {
            whole: set(0),
            restart_bridge: false,
            restart_guard: false,
            content: set(1),
            marker: set(2),
            marker_family: set(3),
            indent: set(4),
            start: set(5),
            end: set(6),
            available: set(7),
            recurrence: set(8),
            priority: set(9),
            state: set(10),
        }
    }

    fn union(&mut self, other: Self) {
        let restart_bridge = self.restart_bridge || other.restart_bridge;
        let restart_guard = self.restart_guard || other.restart_guard;
        *self = Self::from_bits(self.bits() | other.bits());
        self.restart_bridge = restart_bridge;
        self.restart_guard = restart_guard;
    }

    fn any(&self) -> bool {
        self.whole
            || self.content
            || self.marker
            || self.marker_family
            || self.indent
            || self.start
            || self.end
            || self.available
            || self.recurrence
            || self.priority
            || self.state
    }

    /// Keep fields whose authored value must survive a restart before a later
    /// move. The journal is a one-device bridge, not a second conflict
    /// resolver: the destination guard below makes each retained value a
    /// one-shot repair once it has been observed there.
    fn restart_persistent(self) -> Self {
        let whole = self.whole;
        Self {
            whole: whole && (self.state || self.recurrence),
            content: self.content || whole,
            marker: self.marker || whole,
            marker_family: self.marker_family || whole,
            indent: self.indent || whole,
            start: self.start || whole,
            end: self.end || whole,
            available: self.available || whole,
            recurrence: self.recurrence || whole,
            priority: self.priority || whole,
            state: self.state,
            ..Default::default()
        }
    }

    /// `landed` with the fields this device edited taken from `local`.
    fn apply(&self, landed: &Item, local: &Item) -> Item {
        if self.whole {
            let mut merged = local.clone();
            // Occurrence expansion is derived from the recurrence/date fields,
            // not an authored replacement of the item. Preserve occurrence
            // slots that arrived remotely while still letting the acknowledged
            // local value win for slots both copies already share.
            merged.state = merge_occurrence_states(&landed.state, &local.state);
            merged.enforce_marker_constraints();
            return merged;
        }
        let mut merged = landed.clone();
        // Dates, recurrence and occurrence state are only valid on checkbox
        // items. The command that authored one of those fields also promoted
        // the source line to a checkbox, but the compact field mask records
        // only the explicitly requested date/state field. When a moved copy
        // is still a blank line, applying just that mask would make
        // `ReplaceItem` enforce the blank-line invariant and silently strip
        // the authored date again. Carry the dependent marker as part of the
        // same causal bridge when the local value has it.
        let needs_checkbox =
            self.start || self.end || self.available || self.recurrence || self.state;
        if needs_checkbox
            && matches!(local.marker, ItemMarker::Checkbox)
            && !matches!(landed.marker, ItemMarker::Checkbox)
        {
            merged.marker = local.marker;
            merged.marker_family = local.marker_family;
        }
        if self.content {
            merged.content = local.content.clone();
        }
        if self.marker {
            merged.marker = local.marker;
        }
        if self.marker_family {
            merged.marker_family = local.marker_family;
        }
        if self.indent {
            merged.indent = local.indent;
        }
        if self.start {
            merged.start = local.start;
        }
        if self.end {
            merged.end = local.end;
        }
        if self.available {
            merged.available = local.available;
        }
        if self.recurrence {
            merged.repeats = local.repeats.clone();
        }
        if self.priority {
            merged.priority = local.priority;
        }
        if self.state {
            merged.state = local.state.clone();
        }
        if self.restart_bridge {
            // A restarted session retains the complete source snapshot, but
            // its persisted field mask only names what the queued command
            // explicitly changed. Carry untouched source metadata across a
            // move only when the landed copy is the default/missing value;
            // never replace a real concurrent value from another device.
            if !self.content && landed.content.is_empty_text() && !local.content.is_empty_text() {
                merged.content = local.content.clone();
            }
            if !self.marker
                && matches!(landed.marker, ItemMarker::Blank)
                && !matches!(local.marker, ItemMarker::Blank)
            {
                merged.marker = local.marker;
            }
            if !self.marker_family
                && matches!(landed.marker_family, MarkerFamily::Standard)
                && !matches!(local.marker_family, MarkerFamily::Standard)
            {
                merged.marker_family = local.marker_family;
            }
            if !self.indent && landed.indent == 0 && local.indent != 0 {
                merged.indent = local.indent;
            }
            if !self.start && landed.start.is_none() && local.start.is_some() {
                merged.start = local.start;
            }
            if !self.end && landed.end.is_none() && local.end.is_some() {
                merged.end = local.end;
            }
            if !self.available && landed.available.is_none() && local.available.is_some() {
                merged.available = local.available;
            }
            if !self.recurrence && landed.repeats.is_none() && local.repeats.is_some() {
                merged.repeats = local.repeats.clone();
            }
            if !self.priority && landed.priority.is_none() && local.priority.is_some() {
                merged.priority = local.priority;
            }
            let landed_state_is_default = landed.state.iter().all(|state| {
                state.state.progress == 0 && state.state.notification_offset_secs.is_none()
            });
            let local_state_is_non_default = local.state.iter().any(|state| {
                state.state.progress != 0 || state.state.notification_offset_secs.is_some()
            });
            if !self.state && landed_state_is_default && local_state_is_non_default {
                merged.state = local.state.clone();
            }
        }
        // A field mask carries fields one at a time, so it can assemble a
        // combination the model does not allow — a date from a snapshot taken
        // while the line was a checkbox, laid over a line that is now numbered
        // (single-account fuzz seed 10024). Every other writer of an item ends
        // here; this one must too, or the plain workspace holds a value no
        // document can store and the two halves disagree for good.
        merged.enforce_marker_constraints();
        merged
    }

    fn retain_net_changes(&mut self, current: &Item, baseline: Option<&Item>) {
        let Some(baseline) = baseline else {
            return;
        };
        if self.whole {
            if current == baseline {
                *self = Self::default();
            }
            return;
        }
        if self.content && current.content == baseline.content {
            self.content = false;
        }
        if self.marker && current.marker == baseline.marker {
            self.marker = false;
        }
        if self.marker_family && current.marker_family == baseline.marker_family {
            self.marker_family = false;
        }
        if self.indent && current.indent == baseline.indent {
            self.indent = false;
        }
        if self.start && current.start == baseline.start {
            self.start = false;
        }
        if self.end && current.end == baseline.end {
            self.end = false;
        }
        if self.available && current.available == baseline.available {
            self.available = false;
        }
        if self.recurrence && current.repeats == baseline.repeats {
            self.recurrence = false;
        }
        if self.priority && current.priority == baseline.priority {
            self.priority = false;
        }
        if self.state && current.state == baseline.state {
            self.state = false;
        }
    }
}

fn merge_occurrence_states(
    landed: &[OccurrenceState],
    local: &[OccurrenceState],
) -> Vec<OccurrenceState> {
    let mut merged = local.to_vec();
    for remote in landed {
        if !merged
            .iter()
            .any(|state| state.occurrence == remote.occurrence)
        {
            merged.push(remote.clone());
        }
    }
    merged.sort_by(|left, right| left.occurrence.cmp(&right.occurrence));
    merged
}

/// The line edits this device has queued, as they stood just before a sync run
/// lands. See [`AppState::capture_local_item_edits`].
#[derive(Clone)]
struct LocalItemPlacement {
    source: SchemeId,
    scheme: SchemeId,
    position: usize,
    item: Item,
}

#[derive(Clone, Default)]
pub struct LocalItemEdits {
    edits: HashMap<ItemId, (SchemeId, Item, EditedFields)>,
    net_changed: HashSet<ItemId>,
    placements: HashMap<ItemId, LocalItemPlacement>,
}

impl LocalItemEdits {
    pub fn item_ids(&self) -> HashSet<ItemId> {
        self.edits.keys().copied().collect()
    }
}

/// Whether a command changes an item document. Kept exhaustive over the item
/// command family so callers can avoid cloning a potentially large text payload
/// for folder/scheme-only edits.
pub fn command_touches_item(command: &Command) -> bool {
    match command {
        Command::InsertItem { .. }
        | Command::UpdateItemText { .. }
        | Command::ReplaceItem { .. }
        | Command::SetItemIndent { .. }
        | Command::SetItemMarker { .. }
        | Command::SetItemMarkerFamily { .. }
        | Command::SetItemDate { .. }
        | Command::SetItemRecurrence { .. }
        | Command::SetItemPriority { .. }
        | Command::SetOccurrenceNotificationOffset { .. }
        | Command::ToggleOccurrence { .. }
        | Command::DeleteItem { .. }
        | Command::ReorderItem { .. } => true,
        Command::Batch(commands) => commands.iter().any(command_touches_item),
        _ => false,
    }
}

#[derive(Clone, Copy, Default)]
struct EditedSchemeFields {
    name: bool,
    color: bool,
    gsync: bool,
    source: bool,
}

impl EditedSchemeFields {
    fn any(&self) -> bool {
        self.name || self.color || self.gsync || self.source
    }

    fn apply(&self, landed: &mut Scheme, local: &Scheme) {
        if self.name {
            landed.name = local.name.clone();
        }
        if self.color {
            landed.color_index = local.color_index;
        }
        if self.gsync {
            landed.gsync = local.gsync;
        }
        if self.source {
            landed.source = local.source.clone();
        }
    }
}

/// Scheme metadata edits that were still queued when a sync run started.
///
/// The workspace index is materialized as a whole `Workspace` when a pull is
/// landed. That materialization can replace a locally edited scheme with the
/// remote copy before this device's pending index update is pushed. Item edits
/// already have the same capture/reassert protection below; keep scheme
/// metadata under the same boundary so a rename or recolour cannot be silently
/// replaced by the remote snapshot during an ordinary pull-before-push run.
#[derive(Default)]
pub struct LocalSchemeEdits {
    edits: HashMap<SchemeId, (Scheme, EditedSchemeFields)>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct EditedFolderFields {
    pub(crate) name: bool,
    pub(crate) expanded: bool,
    pub(crate) archived: bool,
    /// Restored from durable state after a restart; consumed by one recovery
    /// landing rather than acting as a permanent conflict resolver.
    restart_bridge: bool,
    previous_name: Option<String>,
    previous_expanded: Option<bool>,
    previous_archived: Option<bool>,
    desired_archived: Option<bool>,
    restore_parent: Option<knotq_model::FolderId>,
    restore_position: Option<usize>,
    restore_schemes: Vec<knotq_model::SchemeId>,
}

impl EditedFolderFields {
    fn bits(&self) -> u8 {
        u8::from(self.name) | (u8::from(self.expanded) << 1) | (u8::from(self.archived) << 2)
    }

    fn from_bits(bits: u8) -> Self {
        Self {
            name: bits & 1 != 0,
            expanded: bits & 2 != 0,
            archived: bits & 4 != 0,
            restart_bridge: false,
            previous_name: None,
            previous_expanded: None,
            previous_archived: None,
            desired_archived: None,
            restore_parent: None,
            restore_position: None,
            restore_schemes: Vec::new(),
        }
    }

    fn any(&self) -> bool {
        self.name || self.expanded || self.archived
    }

    fn apply(&self, landed: &mut knotq_model::Folder, local: &knotq_model::Folder) {
        if self.name
            && self
                .previous_name
                .as_ref()
                .is_none_or(|previous| landed.name == *previous)
        {
            landed.name = local.name.clone();
        }
        if self.expanded
            && self
                .previous_expanded
                .is_none_or(|previous| landed.expanded == previous)
        {
            landed.expanded = local.expanded;
        }
    }
}

/// Folder metadata changed by operations that were still pending when a sync
/// run started. Folder expansion is user-visible state in the workspace index;
/// capture it at the same boundary as scheme metadata so a pull cannot replace
/// a local toggle with the remote snapshot's default.
#[derive(Default)]
pub struct LocalFolderEdits {
    edits: HashMap<knotq_model::FolderId, (knotq_model::Folder, EditedFolderFields)>,
}

fn record_scheme(fields: &mut HashMap<SchemeId, EditedSchemeFields>, command: &Command) {
    let mut mark =
        |scheme: SchemeId, set: fn(&mut EditedSchemeFields)| set(fields.entry(scheme).or_default());
    match command {
        Command::RenameScheme { id, .. } => mark(*id, |fields| fields.name = true),
        Command::SetSchemeColor { id, .. } => mark(*id, |fields| fields.color = true),
        Command::SetSchemeGsync { id, .. } => mark(*id, |fields| fields.gsync = true),
        Command::SetSchemeSource { id, .. } => mark(*id, |fields| fields.source = true),
        Command::Batch(commands) => {
            for command in commands {
                record_scheme(fields, command);
            }
        }
        _ => {}
    }
}

fn record_folder(
    fields: &mut HashMap<knotq_model::FolderId, EditedFolderFields>,
    command: &Command,
) {
    let mut mark = |folder: knotq_model::FolderId, set: fn(&mut EditedFolderFields)| {
        set(fields.entry(folder).or_default())
    };
    match command {
        Command::RenameFolder { id, .. } => mark(*id, |fields| fields.name = true),
        Command::SetFolderExpanded { id, .. } => mark(*id, |fields| fields.expanded = true),
        Command::RestoreFolder { folder, .. } => mark(folder.id, |fields| fields.archived = true),
        Command::DeleteFolder { id } => mark(*id, |fields| fields.archived = true),
        Command::Batch(commands) => {
            for command in commands {
                record_folder(fields, command);
            }
        }
        _ => {}
    }
}

impl AppState {
    pub fn record_local_folder_command(&mut self, command: &Command) {
        self.record_local_folder_command_with_inverse(command, None);
    }

    pub(crate) fn record_local_folder_command_with_inverse(
        &mut self,
        command: &Command,
        inverse: Option<&Command>,
    ) {
        let mut fields = HashMap::new();
        record_folder(&mut fields, command);
        record_folder_predecessors(&mut fields, command, inverse);
        if fields.is_empty() {
            return;
        }
        let workspace = self.store.workspace();
        for (folder, edited) in fields {
            let Some(value) = workspace.folders.get(&folder) else {
                continue;
            };
            let mut edited = edited;
            if edited.archived {
                edited.desired_archived = Some(workspace.is_folder_deleted(folder));
                if !workspace.is_folder_deleted(folder) {
                    edited.restore_parent = value.parent;
                    edited.restore_position = value.parent.and_then(|parent| {
                        workspace
                            .folders
                            .get(&parent)?
                            .children
                            .iter()
                            .position(|child| *child == knotq_model::NodeRef::Folder(folder))
                    });
                    edited.restore_schemes =
                        workspace.subtree_scheme_ids(folder).into_iter().collect();
                }
            }
            self.recent_local_folder_edits
                .entry(folder)
                .and_modify(|(current, existing)| {
                    *current = value.clone();
                    existing.name |= edited.name;
                    existing.expanded |= edited.expanded;
                    existing.archived |= edited.archived;
                    if edited.name {
                        existing.previous_name = edited.previous_name.clone();
                    }
                    if edited.expanded {
                        existing.previous_expanded = edited.previous_expanded;
                    }
                    if edited.archived {
                        existing.previous_archived = edited.previous_archived;
                        existing.desired_archived = edited.desired_archived;
                        existing.restore_parent = edited.restore_parent;
                        existing.restore_position = edited.restore_position;
                        existing.restore_schemes = edited.restore_schemes.clone();
                    }
                    // A fresh local command supersedes a restart bridge. Its
                    // pending operation is captured at the next landing.
                    existing.restart_bridge = false;
                })
                .or_insert((value.clone(), edited));
            self.folder_reassertions.remove(&folder);
        }
    }
}

fn record_folder_predecessors(
    fields: &mut HashMap<knotq_model::FolderId, EditedFolderFields>,
    command: &Command,
    inverse: Option<&Command>,
) {
    match (command, inverse) {
        (Command::RenameFolder { id, .. }, Some(Command::RenameFolder { name, .. })) => {
            fields.entry(*id).or_default().previous_name = Some(name.clone())
        }
        (
            Command::SetFolderExpanded { id, .. },
            Some(Command::SetFolderExpanded { expanded, .. }),
        ) => fields.entry(*id).or_default().previous_expanded = Some(*expanded),
        (Command::RestoreFolder { folder, .. }, Some(Command::DeleteFolder { .. })) => {
            fields.entry(folder.id).or_default().previous_archived = Some(true)
        }
        (Command::DeleteFolder { id }, Some(Command::RestoreFolder { .. })) => {
            fields.entry(*id).or_default().previous_archived = Some(false)
        }
        (Command::Batch(commands), Some(Command::Batch(inverses))) => {
            for (command, inverse) in commands.iter().zip(inverses) {
                record_folder_predecessors(fields, command, Some(inverse));
            }
        }
        _ => {}
    }
}

fn record(fields: &mut HashMap<ItemId, EditedFields>, command: &Command) {
    let mut mark = |item: ItemId, set: fn(&mut EditedFields)| set(fields.entry(item).or_default());
    match command {
        Command::InsertItem { item, .. } => mark(item.id, |f| f.whole = true),
        Command::UpdateItemText { item, .. } => mark(*item, |f| f.content = true),
        Command::ReplaceItem { item, .. } => mark(item.id, |f| f.whole = true),
        Command::SetItemIndent { item, .. } => mark(*item, |f| f.indent = true),
        Command::SetItemMarker { item, .. } => mark(*item, |f| f.marker = true),
        Command::SetItemMarkerFamily { item, .. } => mark(*item, |f| f.marker_family = true),
        Command::SetItemDate { item, kind, .. } => match kind {
            DateKind::Start => mark(*item, |f| f.start = true),
            DateKind::End => mark(*item, |f| f.end = true),
            DateKind::Available => mark(*item, |f| f.available = true),
        },
        Command::SetItemRecurrence { item, .. } => mark(*item, |f| f.recurrence = true),
        Command::SetItemPriority { item, .. } => mark(*item, |f| f.priority = true),
        Command::SetOccurrenceNotificationOffset { item, .. }
        | Command::ToggleOccurrence { item, .. } => mark(*item, |f| f.state = true),
        Command::Batch(commands) => {
            for command in commands {
                record(fields, command);
            }
        }
        _ => {}
    }
}

fn collect_deleted_items(command: &Command, deleted: &mut HashSet<ItemId>) {
    match command {
        Command::DeleteItem { item, .. } => {
            deleted.insert(*item);
        }
        Command::Batch(commands) => {
            for command in commands {
                collect_deleted_items(command, deleted);
            }
        }
        _ => {}
    }
}

fn collect_item_schemes(command: &Command, schemes: &mut HashMap<ItemId, SchemeId>) {
    match command {
        Command::InsertItem { scheme, item, .. } | Command::ReplaceItem { scheme, item } => {
            schemes.insert(item.id, *scheme);
        }
        Command::UpdateItemText { scheme, item, .. }
        | Command::SetItemIndent { scheme, item, .. }
        | Command::SetItemMarker { scheme, item, .. }
        | Command::SetItemMarkerFamily { scheme, item, .. }
        | Command::SetItemDate { scheme, item, .. }
        | Command::SetItemRecurrence { scheme, item, .. }
        | Command::SetItemPriority { scheme, item, .. }
        | Command::SetOccurrenceNotificationOffset { scheme, item, .. }
        | Command::ToggleOccurrence { scheme, item, .. }
        | Command::DeleteItem { scheme, item } => {
            schemes.insert(*item, *scheme);
        }
        Command::Batch(commands) => {
            for command in commands {
                collect_item_schemes(command, schemes);
            }
        }
        _ => {}
    }
}

fn collect_move_parts(
    command: &Command,
    deleted: &mut HashMap<ItemId, SchemeId>,
    inserted: &mut Vec<(SchemeId, usize, Item)>,
) {
    match command {
        Command::DeleteItem { scheme, item } => {
            deleted.insert(*item, *scheme);
        }
        Command::InsertItem {
            scheme,
            position,
            item,
        } => inserted.push((*scheme, *position, item.clone())),
        Command::Batch(commands) => {
            for command in commands {
                collect_move_parts(command, deleted, inserted);
            }
        }
        _ => {}
    }
}

fn collect_move_placements(
    command: &Command,
    placements: &mut HashMap<ItemId, LocalItemPlacement>,
) {
    let Command::Batch(_) = command else {
        return;
    };
    let mut deleted = HashMap::new();
    let mut inserted = Vec::new();
    collect_move_parts(command, &mut deleted, &mut inserted);
    for (scheme, position, item) in inserted {
        if deleted
            .get(&item.id)
            .is_some_and(|source| *source != scheme)
        {
            placements.insert(
                item.id,
                LocalItemPlacement {
                    source: *deleted.get(&item.id).expect("move source checked above"),
                    scheme,
                    position,
                    item,
                },
            );
        }
    }
}

/// The insert half of a delete+insert batch is a move between scheme documents.
/// Keep its complete value as move provenance: if a later device materializes
/// the moved copy from an older snapshot, the source device can re-express the
/// value once in the destination document. The one-shot destination guard
/// prevents this bridge from becoming a ping-pong repair loop.
fn record_recent_item_command(
    fields: &mut HashMap<ItemId, EditedFields>,
    command: &Command,
    moved_items: &HashSet<ItemId>,
) {
    let mut mark = |item: ItemId, set: fn(&mut EditedFields)| set(fields.entry(item).or_default());
    match command {
        Command::InsertItem { item, .. } if !moved_items.contains(&item.id) => {
            mark(item.id, |fields| fields.whole = true)
        }
        Command::InsertItem { .. } => {}
        Command::Batch(commands) => {
            for command in commands {
                record_recent_item_command(fields, command, moved_items);
            }
        }
        Command::DeleteItem { .. } => {}
        _ => record(fields, command),
    }
}

fn locate(workspace: &Workspace, item: ItemId) -> Option<(SchemeId, &Item)> {
    workspace
        .schemes
        .iter()
        .filter_map(|(id, scheme)| scheme.item(item).map(|found| (*id, found)))
        .min_by_key(|(id, _)| *id)
}

fn is_default_item_placeholder(item: &Item) -> bool {
    item.content.is_empty_text()
        && matches!(item.marker, ItemMarker::Blank)
        && matches!(item.marker_family, MarkerFamily::Standard)
        && item.indent == 0
        && item.start.is_none()
        && item.end.is_none()
        && item.available.is_none()
        && item.repeats.is_none()
        && item.state.iter().all(|state| {
            state.state.progress == 0 && state.state.notification_offset_secs.is_none()
        })
        && item.priority.is_none()
        && item.external.is_none()
}

impl AppState {
    /// Remember the complete item value after a local edit. A move between
    /// scheme documents can arrive later and make the destination's snapshot
    /// win over the source edit during materialization. Keeping the full value,
    /// rather than only the field named by the last command, preserves the
    /// source copy's already-visible fields too.
    pub fn record_local_item_command(&mut self, command: &Command) {
        if !command_touches_item(command) {
            return;
        }
        let mut fields = HashMap::new();
        let mut moved_items = HashSet::new();
        collect_deleted_items(command, &mut moved_items);
        record_recent_item_command(&mut fields, command, &moved_items);
        let mut preferred_schemes = HashMap::new();
        collect_item_schemes(command, &mut preferred_schemes);
        let workspace = self.store.workspace();
        let current: Vec<_> = fields
            .into_iter()
            .filter_map(|(item, edited)| {
                let preferred = preferred_schemes.get(&item).and_then(|scheme| {
                    workspace
                        .scheme(*scheme)
                        .and_then(|value| value.item(item).map(|found| (*scheme, found)))
                });
                preferred
                    .or_else(|| locate(workspace, item))
                    .map(|(scheme, found)| (item, scheme, found.clone(), edited))
            })
            .collect();
        let tracked = &mut self.recent_local_item_edits.edits;
        for (item, scheme, found, edited) in current {
            self.recent_moved_item_landed_schemes.remove(&item);
            self.recent_moved_item_landed_values.remove(&item);
            match tracked.entry(item) {
                std::collections::hash_map::Entry::Occupied(mut entry) => {
                    // Keep the complete latest value, but accumulate the fields
                    // across successive commands. A later text edit must not
                    // forget an earlier date/marker edit before a stale move
                    // arrives from another device.
                    let preserve_source =
                        (entry.get().2.whole || entry.get().2.content || entry.get().2.indent)
                            && !edited.whole
                            && !edited.content
                            && !edited.indent;
                    if !preserve_source {
                        entry.get_mut().0 = scheme;
                    }
                    // Completion/recurrence maintenance can touch an item
                    // after an earlier whole-value edit has moved into a
                    // second document. When `preserve_source` is true, the
                    // original source stays in the journal; otherwise the
                    // explicit local command's scheme becomes the source.
                    let existing = entry.get().1.clone();
                    entry.get_mut().1 = if is_default_item_placeholder(&existing) {
                        found
                    } else {
                        edited.apply(&existing, &found)
                    };
                    entry.get_mut().2.union(edited);
                    entry.get_mut().2.restart_bridge = false;
                    entry.get_mut().2.restart_guard = false;
                }
                std::collections::hash_map::Entry::Vacant(entry) => {
                    let mut initial = edited;
                    // Occurrence and recurrence edits depend on the item's
                    // complete pre-existing authored value. If this is the
                    // first journal entry after a lazy restore/relaunch,
                    // retain that value too so a stale carry-over cannot
                    // discard marker/date metadata while changing the state.
                    if edited.state || edited.recurrence {
                        initial.whole = true;
                    }
                    entry.insert((scheme, found, initial));
                }
            }
        }
    }

    /// Promote fields recovered from a pre-relaunch queued operation into the
    /// acknowledged journal once that operation has landed. This is the only
    /// safe restart recovery source: unlike the materialized workspace, the
    /// queued record proves the edit was authored on this device.
    pub fn remember_captured_item_edits(&mut self, captured: &LocalItemEdits) {
        if captured.edits.is_empty() {
            return;
        }
        // The journal is user-data provenance, not a visible workspace edit,
        // but it must still be included in the next durable save.
        self.index_dirty = true;
        let tracked = &mut self.recent_local_item_edits.edits;
        for (item, (scheme, value, edited)) in &captured.edits {
            let edited = edited.restart_persistent();
            if !edited.any() {
                continue;
            }
            self.recent_moved_item_landed_schemes.remove(item);
            self.recent_moved_item_landed_values.remove(item);
            match tracked.entry(*item) {
                std::collections::hash_map::Entry::Occupied(mut entry) => {
                    // Keep the source document of an acknowledged edit stable
                    // while a sync landing moves its visible copy. A queued
                    // field record is provenance for the old command, not a
                    // fresh local move; changing this scheme to `locate(...)`
                    // would make the destination look like the source and
                    // disable the bridge that re-expresses the edit.
                    if captured.placements.contains_key(item) {
                        entry.get_mut().0 = *scheme;
                    }
                    let existing = entry.get().1.clone();
                    entry.get_mut().1 = if is_default_item_placeholder(&existing) {
                        value.clone()
                    } else {
                        edited.apply(&existing, value)
                    };
                    entry.get_mut().2.union(edited);
                }
                std::collections::hash_map::Entry::Vacant(entry) => {
                    let mut initial = edited;
                    if edited.state || edited.recurrence {
                        initial.whole = true;
                    }
                    entry.insert((*scheme, value.clone(), initial));
                }
            }
        }
    }

    pub fn restore_recent_item_edits(
        &mut self,
        records: impl IntoIterator<Item = (ItemId, RecentItemEdit)>,
    ) {
        let tracked = &mut self.recent_local_item_edits.edits;
        for (item, record) in records {
            let Ok(value) = serde_json::from_str::<Item>(&record.item) else {
                continue;
            };
            if value.id != item {
                continue;
            }
            let persisted_fields = record.fields;
            let mut edited = EditedFields::from_bits(persisted_fields).restart_persistent();
            // A persisted whole bit is a transport hint for occurrence and
            // recurrence commands, not proof that every item field should be
            // replayed after a restart. The journal retains the complete
            // value separately; reapply the recorded registers explicitly so
            // untouched metadata cannot be overwritten on the original
            // scheme.
            edited.whole = false;
            edited.restart_bridge = persisted_fields & 1 == 0
                && persisted_fields & (1 << 8) == 0
                && persisted_fields & (1 << 10) == 0;
            edited.restart_guard = true;
            if !edited.any() {
                continue;
            }
            tracked.insert(item, (record.scheme, value, edited));
        }
    }

    pub fn recent_item_edits(&self) -> HashMap<ItemId, RecentItemEdit> {
        self.recent_local_item_edits
            .edits
            .iter()
            .map(|(item, (scheme, value, edited))| {
                let edited = edited.restart_persistent();
                (
                    *item,
                    RecentItemEdit {
                        scheme: *scheme,
                        item: serde_json::to_string(value).expect("item journal serializes"),
                        fields: edited.bits(),
                    },
                )
            })
            .collect()
    }

    /// A journal record can survive a restart with only the fields that were
    /// serialized by the queued operation. Before a landing replaces the live
    /// workspace, fill such a placeholder from this device's current copy;
    /// that copy is the last complete local value and is safer than the stale
    /// incoming destination snapshot.
    pub fn hydrate_recent_item_edits(&mut self) {
        let workspace = self.store.workspace();
        let items: Vec<_> = self
            .recent_local_item_edits
            .edits
            .iter()
            .filter_map(|(item, (_, value, _))| {
                is_default_item_placeholder(value)
                    .then(|| locate(workspace, *item).map(|(_, found)| (*item, found.clone())))
                    .flatten()
            })
            .collect();
        for (item, value) in items {
            if let Some((_, current, _)) = self.recent_local_item_edits.edits.get_mut(&item) {
                if !is_default_item_placeholder(&value) {
                    *current = value;
                }
            }
        }
    }

    /// Capture scheme metadata changed by operations that are still pending
    /// before a sync landing clears the operations the run pushed.
    pub fn capture_local_scheme_edits(&self) -> LocalSchemeEdits {
        let mut fields: HashMap<SchemeId, EditedSchemeFields> = HashMap::new();
        for operation in self.store.pending_operations() {
            record_scheme(&mut fields, &operation.command);
        }
        let workspace = self.store.workspace();
        let edits: HashMap<_, _> = fields
            .into_iter()
            .filter(|(_, edited)| edited.any())
            .filter_map(|(scheme, edited)| {
                workspace
                    .schemes
                    .get(&scheme)
                    .map(|scheme| (scheme.id, (scheme.clone(), edited)))
            })
            .collect();
        LocalSchemeEdits { edits }
    }

    /// Re-apply scheme metadata edited locally when a landing materialized an
    /// older remote copy over it. Each field is merged independently so a
    /// remote edit to a different field is retained.
    pub fn reassert_local_scheme_edits(&mut self, captured: LocalSchemeEdits) -> usize {
        let mut commands = Vec::new();
        {
            let workspace = self.store.workspace();
            let mut edits: Vec<_> = captured.edits.into_iter().collect();
            edits.sort_by_key(|(scheme, _)| *scheme);
            for (scheme, (local, fields)) in edits {
                let Some(landed) = workspace.schemes.get(&scheme) else {
                    continue;
                };
                let mut merged = landed.clone();
                fields.apply(&mut merged, &local);
                if merged == *landed {
                    continue;
                }
                if fields.name && merged.name != landed.name {
                    commands.push(Command::RenameScheme {
                        id: scheme,
                        name: merged.name.clone(),
                    });
                }
                if fields.color && merged.color_index != landed.color_index {
                    commands.push(Command::SetSchemeColor {
                        id: scheme,
                        color_index: merged.color_index,
                    });
                }
                if fields.gsync && merged.gsync != landed.gsync {
                    commands.push(Command::SetSchemeGsync {
                        id: scheme,
                        on: merged.gsync,
                    });
                }
                if fields.source && merged.source != landed.source {
                    commands.push(Command::SetSchemeSource {
                        id: scheme,
                        source: merged.source.clone(),
                    });
                }
            }
        }
        let mut applied = 0;
        for command in commands {
            if self
                .apply_prechecked_local_command(command, CommandOrigin::User)
                .is_ok()
            {
                applied += 1;
            }
        }
        applied
    }

    pub fn capture_local_folder_edits(&self, _incoming: &Workspace) -> LocalFolderEdits {
        let mut fields = HashMap::new();
        let pending = self.store.pending_operations();
        for operation in pending {
            record_folder(&mut fields, &operation.command);
        }
        // Only entries restored after a restart are used outside the pending
        // operation boundary. Retaining ordinary acknowledged edits forever
        // would replay an old value over a later legitimate remote edit.
        for (folder, (_, edited)) in &self.recent_local_folder_edits {
            if !edited.restart_bridge && !self.folder_reassertions.contains_key(folder) {
                continue;
            }
            let entry = fields.entry(*folder).or_default();
            entry.name |= edited.name;
            entry.expanded |= edited.expanded;
            entry.archived |= edited.archived;
            entry.restart_bridge |= edited.restart_bridge;
            if entry.previous_name.is_none() {
                entry.previous_name = edited.previous_name.clone();
            }
            if entry.previous_expanded.is_none() {
                entry.previous_expanded = edited.previous_expanded;
            }
            if entry.previous_archived.is_none() {
                entry.previous_archived = edited.previous_archived;
            }
            entry.desired_archived = edited.desired_archived;
            entry.restore_parent = edited.restore_parent;
            entry.restore_position = edited.restore_position;
            entry.restore_schemes = edited.restore_schemes.clone();
        }
        let workspace = self.store.workspace();
        let edits: HashMap<_, _> = fields
            .into_iter()
            .filter(|(_, edited)| edited.any())
            .filter_map(|(folder, mut edited)| {
                workspace.folders.get(&folder).map(|folder| {
                    if edited.archived {
                        edited.desired_archived = Some(workspace.is_folder_deleted(folder.id));
                        if !workspace.is_folder_deleted(folder.id) {
                            edited.restore_parent = folder.parent;
                            edited.restore_position =
                                folder.parent.and_then(|parent| {
                                    workspace.folders.get(&parent)?.children.iter().position(
                                        |child| *child == knotq_model::NodeRef::Folder(folder.id),
                                    )
                                });
                            edited.restore_schemes = workspace
                                .subtree_scheme_ids(folder.id)
                                .into_iter()
                                .collect();
                        }
                    }
                    (folder.id, (folder.clone(), edited))
                })
            })
            .collect();
        LocalFolderEdits { edits }
    }

    pub fn reassert_local_folder_edits(&mut self, captured: LocalFolderEdits) -> usize {
        let mut commands = Vec::new();
        let mut completed = HashSet::new();
        {
            let workspace = self.store.workspace();
            let mut edits: Vec<_> = captured.edits.into_iter().collect();
            edits.sort_by_key(|(folder, _)| *folder);
            for (folder, (local, fields)) in edits {
                let Some(landed) = workspace.folders.get(&folder) else {
                    completed.insert(folder);
                    continue;
                };
                let mut fields = fields;
                // A guarded value only bridges a stale predecessor. If the
                // landed value is already something else, another device has
                // legitimately won this field and this recovery record must
                // not keep participating in future merges.
                if fields.name
                    && fields
                        .previous_name
                        .as_ref()
                        .is_some_and(|previous| landed.name != *previous)
                {
                    fields.name = false;
                    fields.previous_name = None;
                }
                if fields.expanded
                    && fields
                        .previous_expanded
                        .is_some_and(|previous| landed.expanded != previous)
                {
                    fields.expanded = false;
                    fields.previous_expanded = None;
                }
                let landed_archived = workspace.is_folder_deleted(folder);
                if fields.archived
                    && fields
                        .previous_archived
                        .is_some_and(|previous| landed_archived != previous)
                {
                    fields.archived = false;
                    fields.previous_archived = None;
                }
                if !fields.any() {
                    completed.insert(folder);
                    continue;
                }
                let mut merged = landed.clone();
                fields.apply(&mut merged, &local);
                let descendant_restore_needed = fields.desired_archived == Some(false)
                    && fields
                        .restore_schemes
                        .iter()
                        .any(|scheme| workspace.is_scheme_deleted(*scheme));
                let folder_archive_needed = fields
                    .desired_archived
                    .is_some_and(|desired| desired != landed_archived);
                let archive_needed = folder_archive_needed || descendant_restore_needed;
                let guarded = fields.previous_name.is_some()
                    || fields.previous_expanded.is_some()
                    || fields.previous_archived.is_some();
                if (merged != *landed || archive_needed)
                    && self.folder_reassertions.get(&folder).copied().unwrap_or(0) >= 4
                    && !guarded
                {
                    completed.insert(folder);
                    continue;
                }
                if fields.name && merged.name != landed.name {
                    commands.push((
                        folder,
                        Command::RenameFolder {
                            id: folder,
                            name: merged.name.clone(),
                        },
                    ));
                }
                if fields.expanded && merged.expanded != landed.expanded {
                    commands.push((
                        folder,
                        Command::SetFolderExpanded {
                            id: folder,
                            expanded: merged.expanded,
                        },
                    ));
                }
                if folder_archive_needed {
                    if fields.desired_archived == Some(true) {
                        commands.push((folder, Command::DeleteFolder { id: folder }));
                    } else {
                        let parent = fields
                            .restore_parent
                            .or(local.parent)
                            .unwrap_or(workspace.root);
                        let position = fields.restore_position.unwrap_or(0).min(
                            workspace.folders.get(&parent).map_or(0, |parent_folder| {
                                parent_folder
                                    .children
                                    .iter()
                                    .filter(|child| **child != knotq_model::NodeRef::Folder(folder))
                                    .count()
                            }),
                        );
                        commands.push((
                            folder,
                            Command::RestoreFolder {
                                parent,
                                position,
                                folder: local.clone(),
                            },
                        ));
                    }
                }
                if descendant_restore_needed {
                    let position = workspace
                        .folders
                        .get(&folder)
                        .map_or(0, |folder| folder.children.len());
                    for scheme_id in &fields.restore_schemes {
                        let Some(scheme) = workspace.schemes.get(scheme_id).cloned() else {
                            continue;
                        };
                        if workspace.is_scheme_deleted(*scheme_id) {
                            commands.push((
                                folder,
                                Command::RestoreScheme {
                                    folder,
                                    position,
                                    scheme,
                                },
                            ));
                        }
                    }
                }
                if merged == *landed && !archive_needed && !guarded {
                    // An unguarded bridge has landed its value, so it must not
                    // replay forever. Guarded entries retain their predecessor
                    // so a later stale landing can be repaired safely without
                    // overwriting a legitimate remote edit to another value.
                    completed.insert(folder);
                } else {
                    *self.folder_reassertions.entry(folder).or_default() += 1;
                }
            }
        }
        let previous_suppression = self.suppress_local_folder_journal;
        self.suppress_local_folder_journal = true;
        let mut applied = 0;
        let mut expected = HashMap::new();
        for (folder, _) in &commands {
            *expected.entry(*folder).or_insert(0usize) += 1;
        }
        let mut succeeded = HashMap::new();
        for (folder, command) in commands {
            if !matches!(
                &command,
                Command::RenameFolder { .. }
                    | Command::SetFolderExpanded { .. }
                    | Command::DeleteFolder { .. }
                    | Command::RestoreFolder { .. }
                    | Command::RestoreScheme { .. }
            ) {
                continue;
            }
            if self
                .apply_prechecked_local_command(command.clone(), CommandOrigin::User)
                .is_ok()
            {
                applied += 1;
                *succeeded.entry(folder).or_insert(0usize) += 1;
            }
        }
        self.suppress_local_folder_journal = previous_suppression;
        for (folder, count) in expected {
            if succeeded.get(&folder) == Some(&count) {
                completed.insert(folder);
            }
        }
        for folder in completed {
            self.recent_local_folder_edits.remove(&folder);
            self.folder_reassertions.remove(&folder);
        }
        applied
    }

    /// Restore acknowledged folder metadata retained in the durable sync
    /// state. The complete current folder is used as the baseline, and only
    /// the explicitly-authored fields are replaced by the journal value.
    pub fn restore_recent_folder_edits(
        &mut self,
        records: impl IntoIterator<Item = (knotq_model::FolderId, knotq_sync::RecentFolderEdit)>,
    ) {
        let workspace = self.store.workspace();
        for (folder, record) in records {
            let Some(current) = workspace.folders.get(&folder) else {
                continue;
            };
            let fields = EditedFolderFields::from_bits(record.fields);
            if !fields.any() {
                continue;
            }
            let mut value = current.clone();
            if fields.name {
                value.name = record.name;
            }
            if fields.expanded {
                value.expanded = record.expanded;
            }
            let mut fields = fields;
            fields.restart_bridge = true;
            fields.previous_name = record.previous_name;
            fields.previous_expanded = record.previous_expanded;
            fields.previous_archived = record.previous_archived;
            fields.desired_archived = fields.archived.then_some(record.archived);
            fields.restore_parent = record.restore_parent;
            fields.restore_position = record.restore_position;
            fields.restore_schemes = record.restore_schemes;
            self.recent_local_folder_edits
                .insert(folder, (value, fields));
        }
    }

    /// Snapshot acknowledged folder metadata for the durable sync state.
    pub fn recent_folder_edits(
        &self,
    ) -> HashMap<knotq_model::FolderId, knotq_sync::RecentFolderEdit> {
        self.recent_local_folder_edits
            .iter()
            .map(|(folder, (value, fields))| {
                (
                    *folder,
                    knotq_sync::RecentFolderEdit {
                        folder: *folder,
                        name: value.name.clone(),
                        expanded: value.expanded,
                        fields: fields.bits(),
                        previous_name: fields.previous_name.clone(),
                        previous_expanded: fields.previous_expanded,
                        archived: fields
                            .desired_archived
                            .unwrap_or_else(|| self.store.workspace().is_folder_deleted(*folder)),
                        previous_archived: fields.previous_archived,
                        restore_parent: fields.restore_parent,
                        restore_position: fields.restore_position,
                        restore_schemes: fields.restore_schemes.clone(),
                    },
                )
            })
            .collect()
    }

    /// Record which fields of which lines this device's queued operations edit,
    /// with each line's scheme and value right now. Call it before a landing
    /// clears the operations the run pushed.
    pub fn capture_local_item_edits(
        &self,
        queued: &[QueuedItemFields],
        baseline: &Workspace,
    ) -> LocalItemEdits {
        let mut fields: HashMap<ItemId, EditedFields> = HashMap::new();
        let mut must_reassert = HashSet::new();
        let mut preferred_schemes = HashMap::new();
        let mut placements = HashMap::new();
        for operation in self.store.pending_operations() {
            let mut moved_items = HashSet::new();
            collect_deleted_items(&operation.command, &mut moved_items);
            record_recent_item_command(&mut fields, &operation.command, &moved_items);
            collect_item_schemes(&operation.command, &mut preferred_schemes);
            collect_move_placements(&operation.command, &mut placements);
            must_reassert.extend(fields.keys().copied());
        }
        // Edits from before a relaunch are no longer store operations; the sync
        // run hands back the field records persisted with them.
        for record in queued {
            if let Ok(item) = record.item.parse::<ItemId>() {
                must_reassert.insert(item);
                fields
                    .entry(item)
                    .or_default()
                    .union(EditedFields::from_bits(record.fields));
            }
        }
        for edited in fields.values_mut() {
            if edited.state || edited.recurrence {
                edited.whole = true;
            }
        }
        let workspace = self.store.workspace();
        let edits: HashMap<_, _> = fields
            .into_iter()
            .filter(|(_, edited)| edited.any())
            .filter_map(|(item, edited)| {
                // A queued field mask can outlive the workspace snapshot that
                // produced it. After a relaunch, that snapshot may still be
                // the old copy of a line even though the journal already
                // contains this device's newer value. The journal is updated
                // synchronously with every accepted local item command, so it
                // is the authoritative value source whenever it exists;
                // queued/workspace state supplies the value only for items
                // without a journal entry yet.
                self.recent_local_item_edits
                    .edits
                    .get(&item)
                    .map(|(scheme, value, _)| (*scheme, value))
                    .or_else(|| {
                        preferred_schemes.get(&item).and_then(|scheme| {
                            workspace
                                .scheme(*scheme)
                                .and_then(|value| value.item(item).map(|found| (*scheme, found)))
                        })
                    })
                    .or_else(|| locate(workspace, item))
                    .map(|(scheme, found)| {
                        let mut net_fields = edited;
                        net_fields.retain_net_changes(
                            found,
                            locate(baseline, item).map(|(_, baseline)| baseline),
                        );
                        (item, (scheme, found.clone(), edited))
                    })
            })
            .collect();
        let net_changed = edits
            .iter()
            .filter_map(|(item, (_, current, edited))| {
                let baseline_item = locate(baseline, *item).map(|(_, item)| item);
                let mut net_fields = *edited;
                net_fields.retain_net_changes(current, baseline_item);
                (net_fields.any() || must_reassert.contains(item)).then_some(*item)
            })
            .collect();
        LocalItemEdits {
            edits,
            net_changed,
            placements,
        }
    }

    /// Which line fields each queued store operation edits, for persisting with
    /// the pending queue so the record survives a relaunch.
    pub fn queued_item_fields(&self) -> HashMap<OperationId, Vec<QueuedItemFields>> {
        let mut out = HashMap::new();
        // A deferred CRDT flush attaches its updates to the NEWEST operation
        // (`WorkspaceStore::flush_crdt`), so one queued edit's bytes can carry
        // the commands of every operation since the previous flush. Recording
        // only an operation's own command loses the rest of them — the edit is
        // queued, but nothing says which fields it changed (deep production
        // fuzz, seed 10000: a line retyped two commands before a relaunch).
        let mut pending: HashMap<ItemId, EditedFields> = HashMap::new();
        for operation in self.store.pending_operations() {
            let mut moved_items = HashSet::new();
            collect_deleted_items(&operation.command, &mut moved_items);
            record_recent_item_command(&mut pending, &operation.command, &moved_items);
            if operation.crdt_updates.is_empty() {
                continue;
            }
            let mut records: Vec<QueuedItemFields> = std::mem::take(&mut pending)
                .into_iter()
                .filter(|(_, edited)| edited.any())
                .map(|(item, edited)| QueuedItemFields {
                    item: item.to_string(),
                    fields: edited.bits(),
                })
                .collect();
            if !records.is_empty() {
                records.sort_by(|left, right| left.item.cmp(&right.item));
                out.entry(operation.id)
                    .and_modify(|existing: &mut Vec<QueuedItemFields>| {
                        existing.extend(records.iter().cloned())
                    })
                    .or_insert(records);
            }
        }
        out
    }

    /// After a sync landed, re-apply fields edited locally before the run if the
    /// landed copy lost them. This covers both cross-scheme moves and replace
    /// fallbacks that reverted an item in place.
    pub fn reassert_local_item_edits(&mut self, captured: LocalItemEdits) -> usize {
        let mut captured = captured;
        // A relaunch can restore the queued field mask while the live copy
        // used to build that capture is only an id/default placeholder. The
        // acknowledged journal is the richer source captured before the
        // landing; use it for this pending bridge too, otherwise the pending
        // path would suppress the journal path and reassert defaults.
        for (item, (_, value, _)) in &mut captured.edits {
            if is_default_item_placeholder(value) {
                if let Some((_, journal, _)) = self.recent_local_item_edits.edits.get(item) {
                    if !is_default_item_placeholder(journal) {
                        *value = journal.clone();
                    }
                }
            }
        }
        let placements = std::mem::take(&mut captured.placements);
        let skip_items = HashSet::new();
        let fields = self.reassert_item_edits(captured, false, &skip_items);
        fields + self.reassert_local_item_placements(placements)
    }

    fn reassert_local_item_placements(
        &mut self,
        placements: HashMap<ItemId, LocalItemPlacement>,
    ) -> usize {
        let workspace = self.store.workspace();
        let mut commands = Vec::new();
        let mut ordered: Vec<_> = placements.into_values().collect();
        ordered.sort_by_key(|placement| placement.item.id);
        for placement in ordered {
            let Some(target) = workspace.scheme(placement.scheme) else {
                continue;
            };
            if workspace.is_scheme_read_only(placement.scheme)
                || workspace.is_scheme_deleted(placement.scheme)
            {
                continue;
            }
            let target_has_item = target.item(placement.item.id).is_some();
            if placement.source != placement.scheme
                && workspace
                    .scheme(placement.source)
                    .is_some_and(|scheme| scheme.item(placement.item.id).is_some())
            {
                // Reassert only the source deletion authored by this move.
                // Other copies can be legitimate concurrent inserts; deleting
                // every copy would turn a placement repair into data loss.
                commands.push(Command::DeleteItem {
                    scheme: placement.source,
                    item: placement.item.id,
                });
            }
            if !target_has_item {
                commands.push(Command::InsertItem {
                    scheme: placement.scheme,
                    position: placement.position.min(target.items.len()),
                    item: placement.item,
                });
            }
        }
        if commands.is_empty() {
            return 0;
        }
        let previous_suppression = self.suppress_local_item_journal;
        self.suppress_local_item_journal = true;
        let applied = self
            .apply_prechecked_local_command(Command::Batch(commands), CommandOrigin::User)
            .is_ok() as usize;
        self.suppress_local_item_journal = previous_suppression;
        applied
    }

    /// Re-apply complete locally-authored item values only when the landed copy
    /// changed scheme. This is the acknowledged-edit case: pending-operation
    /// capture alone is insufficient because the source edit may have been
    /// pushed and cleared before the remote move arrives.
    pub fn reassert_recent_moved_item_edits(&mut self, skip_items: &HashSet<ItemId>) -> usize {
        self.reassert_item_edits(self.recent_local_item_edits.clone(), true, skip_items)
    }

    fn reassert_item_edits(
        &mut self,
        captured: LocalItemEdits,
        moved_only: bool,
        skip_items: &HashSet<ItemId>,
    ) -> usize {
        let mut commands = Vec::new();
        let mut observed_destinations = Vec::new();
        let mut consumed_restart_bridges = HashSet::new();
        // Items whose cross-document bridge is authored below. A bridge is a
        // ONE-SHOT repair, not a standing claim: once this device has written
        // its authored value into the destination document, that document owns
        // it and Yrs resolves every later conflict there causally. Keeping the
        // journal entry alive turns the journal into a second, non-causal
        // resolver — two devices each re-assert their retained snapshot on
        // every landing, so one repair is authored per sync for ever and the
        // pending queue never drains (production fuzz seeds 10004/10005,
        // reported as "wedged: 1 unpushed edit after settling"). So the entry
        // is retired once its repair has actually been applied.
        let mut spent_bridges: HashSet<ItemId> = HashSet::new();
        {
            let workspace = self.store.workspace();
            let mut items: Vec<_> = captured.edits.into_iter().collect();
            items.sort_by_key(|(item, _)| *item);
            for (item, (local_scheme, local, edited)) in items {
                if moved_only && skip_items.contains(&item) {
                    continue;
                }
                let Some((landed_scheme, landed)) = locate(workspace, item) else {
                    if moved_only {
                        continue;
                    }
                    // A locally inserted item can be absent when the landing
                    // materialized a competing scheme snapshot. Recreate it at
                    // the end of its original editable scheme; the item id is
                    // stable, so the CRDT insert is idempotent on other devices.
                    if local.external.is_none()
                        && workspace.scheme(local_scheme).is_some()
                        && !workspace.is_scheme_read_only(local_scheme)
                        && !workspace.is_scheme_deleted(local_scheme)
                    {
                        let position = workspace
                            .scheme(local_scheme)
                            .map(|scheme| scheme.items.len())
                            .unwrap_or_default();
                        commands.push(Command::InsertItem {
                            scheme: local_scheme,
                            position,
                            item: local,
                        });
                    }
                    continue;
                };
                if !moved_only
                    && captured.net_changed.contains(&item)
                    && landed_scheme != local_scheme
                {
                    // A local edit can coexist briefly with both halves of a
                    // concurrent move in the live workspace. Reassert the
                    // authored fields only into a copy that landed in a
                    // different document. A same-document conflict already
                    // has Yrs' causal/register resolution; writing the stale
                    // pre-landing value back there would create a new local
                    // operation on every pull and make two legitimate edits
                    // ping-pong forever.
                    for (scheme_id, scheme) in &workspace.schemes {
                        let Some(current) = scheme.item(item) else {
                            continue;
                        };
                        if workspace.is_scheme_read_only(*scheme_id) {
                            continue;
                        }
                        let merged = edited.apply(current, &local);
                        if merged != *current {
                            commands.push(Command::ReplaceItem {
                                scheme: *scheme_id,
                                item: merged,
                            });
                        }
                    }
                    continue;
                }
                if moved_only {
                    // A retained acknowledged edit is a bridge from the
                    // source document into a different destination document.
                    // Once the item is back in its source document, or two
                    // devices have independently edited the same destination,
                    // ordinary Yrs conflict resolution is authoritative. A
                    // journal replay there would be a second, non-causal
                    // conflict resolver and can make two valid edits ping-pong
                    // forever.
                    // A same-document conflict is already resolved by Yrs,
                    // even when the destination value has changed since this
                    // journal last observed it. Replaying the retained source
                    // snapshot there manufactures a fresh local write on every
                    // landing — a second, non-causal conflict resolver racing
                    // the first — and the queue never drains (production fuzz
                    // seeds 10004/10005: one `ReplaceItem` re-authored per
                    // settle round, for ever). The journal's job is the
                    // cross-document case only: a move copies the line into
                    // another scheme's document, where this device's edit to
                    // the source copy landed on a line that no longer exists.
                    if landed_scheme == local_scheme {
                        observed_destinations.push((item, landed_scheme, landed.clone()));
                        continue;
                    } else if self
                        .recent_moved_item_landed_schemes
                        .get(&item)
                        .is_some_and(|scheme| *scheme == landed_scheme)
                        && !self
                            .recent_moved_item_landed_values
                            .get(&item)
                            .is_some_and(|previous| previous != landed && edited.whole)
                    {
                        // This destination has already been observed for this
                        // journal generation. It may now contain a concurrent
                        // edit from another device; replaying the source
                        // snapshot again would manufacture a ping-pong loop.
                        // A new local edit clears this observation, and a
                        // genuinely new destination is still bridged once.
                        observed_destinations.push((item, landed_scheme, landed.clone()));
                        continue;
                    }
                    observed_destinations.push((item, landed_scheme, landed.clone()));
                }
                if !moved_only && landed_scheme == local_scheme {
                    continue;
                }
                if landed.external.is_some()
                    || workspace.is_scheme_read_only(landed_scheme)
                    || (!moved_only && workspace.is_scheme_deleted(landed_scheme))
                {
                    continue;
                }
                if moved_only && edited.restart_guard && landed_scheme != local_scheme {
                    consumed_restart_bridges.insert(item);
                }
                let merged = edited.apply(landed, &local);
                // A moved line can already look correct in the destination
                // workspace while the destination CRDT still carries a stale
                // snapshot behind its field-wise view.  A later metadata-only
                // write can make that hidden snapshot win for content/indent
                // (the visible line then jumps back even though nobody edited
                // those fields).  Re-express the complete authored value once
                // on a genuinely new destination so the destination document
                // has the same causal snapshot as the visible bridge.
                let first_destination_bridge = moved_only
                    && landed_scheme != local_scheme
                    && !self.recent_moved_item_landed_schemes.contains_key(&item)
                    && edited.whole;
                let changed_after_destination_bridge = moved_only
                    && self
                        .recent_moved_item_landed_values
                        .get(&item)
                        .is_some_and(|previous| {
                            previous != landed && (edited.whole || edited.content || edited.indent)
                        });
                if merged != *landed || first_destination_bridge || changed_after_destination_bridge
                {
                    if moved_only && landed_scheme != local_scheme {
                        spent_bridges.insert(item);
                    }
                    commands.push(Command::ReplaceItem {
                        scheme: landed_scheme,
                        item: merged,
                    });
                }
            }
        }
        for (item, scheme, landed) in observed_destinations {
            self.recent_moved_item_landed_schemes.insert(item, scheme);
            self.recent_moved_item_landed_values.insert(item, landed);
        }
        if !consumed_restart_bridges.is_empty() {
            self.index_dirty = true;
            for item in consumed_restart_bridges {
                if let Some((_, _, edited)) = self.recent_local_item_edits.edits.get_mut(&item) {
                    edited.restart_bridge = false;
                    edited.restart_guard = false;
                }
            }
        }
        // One command per line: a line whose re-apply is refused must not take
        // the others down with it.
        let mut applied = 0;
        let mut applied_items: HashSet<ItemId> = HashSet::new();
        let previous_suppression = self.suppress_local_item_journal;
        self.suppress_local_item_journal = true;
        for command in commands {
            let item = match &command {
                Command::ReplaceItem { item, .. } => item.id,
                _ => continue,
            };
            match self.apply_prechecked_local_command(command.clone(), CommandOrigin::User) {
                Ok(_) => {
                    applied += 1;
                    applied_items.insert(item);
                    if moved_only {
                        if let Command::ReplaceItem { scheme, item } = &command {
                            if let Some(current) = self
                                .store
                                .workspace()
                                .scheme(*scheme)
                                .and_then(|scheme| scheme.item(item.id))
                            {
                                // The bridge guard records the value after its
                                // repair, not the stale value that triggered
                                // it. Otherwise the next no-op sync would see
                                // its own repair as a new remote divergence.
                                self.recent_moved_item_landed_schemes
                                    .insert(item.id, *scheme);
                                self.recent_moved_item_landed_values
                                    .insert(item.id, current.clone());
                            }
                        }
                    }
                }
                Err(err) => eprintln!(
                    "sync: could not re-apply this device's edit to moved line {item}: {err:?}"
                ),
            }
        }
        self.suppress_local_item_journal = previous_suppression;
        // Retire the journal entries whose bridge actually landed. A command
        // that was refused keeps its entry so the repair is retried.
        spent_bridges.retain(|item| applied_items.contains(item));
        if !spent_bridges.is_empty() {
            self.index_dirty = true;
            for item in &spent_bridges {
                self.recent_local_item_edits.edits.remove(item);
                self.recent_moved_item_landed_schemes.remove(item);
                self.recent_moved_item_landed_values.remove(item);
            }
        }
        applied
    }
}
