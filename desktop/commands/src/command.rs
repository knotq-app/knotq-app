use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use knotq_model::{
    DeletedFolderOrigin, DeletedSchemeOrigin, Folder, FolderId, Item, ItemId, ItemMarker, NodeRef,
    OccurrenceId, Recurrence, Scheme, SchemeId, SchemeSource,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DateKind {
    Start,
    End,
    Available,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Command {
    CreateFolder {
        parent: FolderId,
        name: String,
        position: Option<usize>,
    },
    RestoreFolder {
        parent: FolderId,
        position: usize,
        folder: Folder,
    },
    RestoreDeletedFolder {
        folder: FolderId,
        position: usize,
        folders: Vec<Folder>,
        schemes: Vec<Scheme>,
        origin: Option<DeletedFolderOrigin>,
    },
    RenameFolder {
        id: FolderId,
        name: String,
    },
    SetFolderExpanded {
        id: FolderId,
        expanded: bool,
    },
    DeleteFolder {
        id: FolderId,
    },
    PermanentlyDeleteFolder {
        id: FolderId,
    },

    CreateScheme {
        folder: FolderId,
        name: String,
        color_index: u8,
        position: Option<usize>,
    },
    RestoreScheme {
        folder: FolderId,
        position: usize,
        scheme: Scheme,
    },
    RestoreDeletedScheme {
        position: usize,
        scheme: Scheme,
        origin: Option<DeletedSchemeOrigin>,
    },
    RenameScheme {
        id: SchemeId,
        name: String,
    },
    SetSchemeColor {
        id: SchemeId,
        color_index: u8,
    },
    SetSchemeGsync {
        id: SchemeId,
        on: bool,
    },
    SetSchemeSource {
        id: SchemeId,
        source: SchemeSource,
    },
    DeleteScheme {
        id: SchemeId,
    },
    PermanentlyDeleteScheme {
        id: SchemeId,
    },

    MoveNode {
        node: NodeRef,
        new_parent: FolderId,
        position: usize,
    },

    InsertItem {
        scheme: SchemeId,
        position: usize,
        item: Item,
    },
    UpdateItemText {
        scheme: SchemeId,
        item: ItemId,
        text: String,
    },
    ReplaceItem {
        scheme: SchemeId,
        item: Item,
    },
    SetItemIndent {
        scheme: SchemeId,
        item: ItemId,
        indent: u8,
    },
    SetItemMarker {
        scheme: SchemeId,
        item: ItemId,
        marker: ItemMarker,
    },
    SetItemDate {
        scheme: SchemeId,
        item: ItemId,
        kind: DateKind,
        date: Option<DateTime<Utc>>,
    },
    SetItemRecurrence {
        scheme: SchemeId,
        item: ItemId,
        repeats: Option<Recurrence>,
    },
    SetItemPriority {
        scheme: SchemeId,
        item: ItemId,
        priority: Option<u8>,
    },
    SetOccurrenceNotificationOffset {
        scheme: SchemeId,
        item: ItemId,
        occurrence: OccurrenceId,
        offset_secs: Option<i64>,
    },
    ToggleOccurrence {
        scheme: SchemeId,
        item: ItemId,
        occurrence: OccurrenceId,
    },
    DeleteItem {
        scheme: SchemeId,
        item: ItemId,
    },
    ReorderItem {
        scheme: SchemeId,
        from: usize,
        to: usize,
    },

    Batch(Vec<Command>),
}

impl Command {
    /// Collapses a `Vec<Command>` into a single `Command`, returning `None` for
    /// an empty vec, the single element for a one-element vec, or `Batch` otherwise.
    pub fn from_vec(mut commands: Vec<Command>) -> Option<Command> {
        match commands.len() {
            0 => None,
            1 => Some(commands.remove(0)),
            _ => Some(Command::Batch(commands)),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandOrigin {
    User,
    Importer,
    Migration,
}
