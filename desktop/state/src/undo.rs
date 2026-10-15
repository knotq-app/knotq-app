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

/// The clock undo grouping reads. Wall time in the app; frozen by
/// [`freeze_undo_clock`] so a test that drives edits programmatically groups
/// them the same way however fast the machine runs it. With the real clock, a
/// loaded machine spread the production fuzzer's steps past the grouping window
/// and the same seed played a different scenario under load than on an idle
/// machine, which is why some fuzz seeds failed only while other runs shared the
/// CPU.
pub fn undo_clock_now() -> Instant {
    FROZEN_UNDO_CLOCK
        .get()
        .copied()
        .unwrap_or_else(Instant::now)
}

static FROZEN_UNDO_CLOCK: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

/// Stop the undo-grouping clock for the rest of the process: every edit then
/// counts as made at the same instant. For tests only.
pub fn freeze_undo_clock() {
    let _ = FROZEN_UNDO_CLOCK.set(Instant::now());
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
