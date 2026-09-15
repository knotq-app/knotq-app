//! Keeping a line edit when another device moves the line to another scheme.
//!
//! Each scheme is its own CRDT document, so moving a line between schemes is a
//! delete in the source and a fresh copy in the target — a copy of the line as
//! the moving device saw it (carry-over into today's page is exactly this). An
//! edit this device made to the source copy meanwhile lands on a deleted line
//! and is lost for every device. When a sync brings such a move in, this device
//! still knows which fields it edited, so it re-applies them to the moved line
//! as a new edit, which every device then converges on.

use std::collections::HashMap;

use knotq_commands::{Command, CommandOrigin, DateKind};
use knotq_model::{Item, ItemId, SchemeId, Workspace};

use crate::AppState;

#[derive(Clone, Copy, Default)]
struct EditedFields {
    whole: bool,
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

    /// `landed` with the fields this device edited taken from `local`.
    fn apply(&self, landed: &Item, local: &Item) -> Item {
        if self.whole {
            return local.clone();
        }
        let mut merged = landed.clone();
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
        merged
    }
}

/// The line edits this device has queued, as they stood just before a sync run
/// lands. See [`AppState::capture_local_item_edits`].
#[derive(Default)]
pub struct LocalItemEdits {
    edits: HashMap<ItemId, (SchemeId, Item, EditedFields)>,
}

fn record(fields: &mut HashMap<ItemId, EditedFields>, command: &Command) {
    let mut mark = |item: ItemId, set: fn(&mut EditedFields)| set(fields.entry(item).or_default());
    match command {
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

fn locate(workspace: &Workspace, item: ItemId) -> Option<(SchemeId, &Item)> {
    workspace
        .schemes
        .iter()
        .find_map(|(id, scheme)| scheme.item(item).map(|found| (*id, found)))
}

impl AppState {
    /// Record which fields of which lines this device's queued operations edit,
    /// with each line's scheme and value right now. Call it before a landing
    /// clears the operations the run pushed.
    pub fn capture_local_item_edits(&self) -> LocalItemEdits {
        let mut fields: HashMap<ItemId, EditedFields> = HashMap::new();
        for operation in self.store.pending_operations() {
            record(&mut fields, &operation.command);
        }
        let workspace = self.store.workspace();
        let edits = fields
            .into_iter()
            .filter(|(_, edited)| edited.any())
            .filter_map(|(item, edited)| {
                locate(workspace, item)
                    .map(|(scheme, found)| (item, (scheme, found.clone(), edited)))
            })
            .collect();
        LocalItemEdits { edits }
    }

    /// After a sync landed: a line this device edited that now sits in a
    /// different scheme — another device moved it, carrying its own copy of the
    /// line — gets the fields this device edited re-applied there. Returns how
    /// many lines were re-applied.
    pub fn reassert_local_item_edits(&mut self, captured: LocalItemEdits) -> usize {
        let mut commands = Vec::new();
        {
            let workspace = self.store.workspace();
            let mut items: Vec<_> = captured.edits.into_iter().collect();
            items.sort_by_key(|(item, _)| *item);
            for (item, (local_scheme, local, edited)) in items {
                let Some((landed_scheme, landed)) = locate(workspace, item) else {
                    continue;
                };
                if landed_scheme == local_scheme
                    || landed.external.is_some()
                    || workspace.is_scheme_read_only(landed_scheme)
                    || workspace.is_scheme_deleted(landed_scheme)
                {
                    continue;
                }
                let merged = edited.apply(landed, &local);
                if merged != *landed {
                    commands.push(Command::ReplaceItem {
                        scheme: landed_scheme,
                        item: merged,
                    });
                }
            }
        }
        let count = commands.len();
        if count > 0
            && self
                .apply_prechecked_local_command(Command::Batch(commands), CommandOrigin::User)
                .is_err()
        {
            return 0;
        }
        count
    }
}
