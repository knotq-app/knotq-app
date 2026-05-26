use std::collections::VecDeque;
use std::time::{Duration as StdDuration, Instant};

use crate::CalendarOccurrenceKey;
use knotq_commands::Command;

pub const EDITOR_UNDO_GROUP_WINDOW: StdDuration = StdDuration::from_millis(1500);
pub const UNDO_DEPTH: usize = 200;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct EditorUndoKey {
    pub scheme_id: knotq_model::SchemeId,
    pub item_id: knotq_model::ItemId,
}

#[derive(Clone, Copy, Debug)]
pub struct EditorUndoGroup {
    pub key: EditorUndoKey,
    pub last_edit: Instant,
}

#[derive(Clone, Debug)]
pub struct UndoRedoStack {
    undo: VecDeque<Command>,
    redo: VecDeque<Command>,
    max_depth: usize,
}

impl Default for UndoRedoStack {
    fn default() -> Self {
        Self::new(UNDO_DEPTH)
    }
}

impl UndoRedoStack {
    pub fn new(max_depth: usize) -> Self {
        Self {
            undo: VecDeque::new(),
            redo: VecDeque::new(),
            max_depth,
        }
    }

    pub fn push_undo(&mut self, command: Command) {
        self.undo.push_back(command);
        while self.undo.len() > self.max_depth {
            self.undo.pop_front();
        }
    }

    pub fn pop_undo(&mut self) -> Option<Command> {
        self.undo.pop_back()
    }

    pub fn push_redo(&mut self, command: Command) {
        self.redo.push_back(command);
    }

    pub fn pop_redo(&mut self) -> Option<Command> {
        self.redo.pop_back()
    }

    pub fn clear_redo(&mut self) {
        self.redo.clear();
    }

    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    pub fn redo_len(&self) -> usize {
        self.redo.len()
    }
}

pub fn editor_undo_key(cmd: &Command) -> Option<EditorUndoKey> {
    match cmd {
        Command::UpdateItemText { scheme, item, .. } => Some(EditorUndoKey {
            scheme_id: *scheme,
            item_id: *item,
        }),
        _ => None,
    }
}

pub fn recurrence_undo_key(cmd: &Command) -> Option<EditorUndoKey> {
    match cmd {
        Command::SetItemRecurrence { scheme, item, .. } => Some(EditorUndoKey {
            scheme_id: *scheme,
            item_id: *item,
        }),
        _ => None,
    }
}

pub fn should_coalesce_editor_undo(
    key: Option<EditorUndoKey>,
    group: Option<EditorUndoGroup>,
    now: Instant,
) -> bool {
    key.is_some_and(|key| {
        group.is_some_and(|group| {
            group.key == key
                && now.saturating_duration_since(group.last_edit) <= EDITOR_UNDO_GROUP_WINDOW
        })
    })
}

pub fn should_coalesce_recurrence_undo(
    key: Option<EditorUndoKey>,
    group: Option<EditorUndoGroup>,
    active_repeat_popover_key: Option<EditorUndoKey>,
) -> bool {
    key.is_some_and(|key| {
        group.is_some_and(|group| group.key == key)
            && active_repeat_popover_key.is_some_and(|active| active == key)
    })
}

pub fn calendar_toggle_keys(cmd: &Command) -> Vec<CalendarOccurrenceKey> {
    let mut keys = Vec::new();
    collect_calendar_toggle_keys(cmd, &mut keys);
    keys
}

fn collect_calendar_toggle_keys(cmd: &Command, keys: &mut Vec<CalendarOccurrenceKey>) {
    match cmd {
        Command::ToggleOccurrence {
            scheme,
            item,
            occurrence,
        } => keys.push(CalendarOccurrenceKey {
            scheme_id: *scheme,
            item_id: *item,
            occurrence: occurrence.clone(),
        }),
        Command::Batch(cmds) => {
            for cmd in cmds {
                collect_calendar_toggle_keys(cmd, keys);
            }
        }
        _ => {}
    }
}
