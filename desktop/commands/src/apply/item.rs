use knotq_model::{Item, ItemId, ItemMarker, OccurrenceId, Recurrence, SchemeId, Workspace};

use crate::invariants::CommandError;
use crate::{ChangeSet, Command, CommandReceipt, DateKind};

pub(crate) fn apply_item(
    workspace: &mut Workspace,
    cmd: Command,
) -> Result<CommandReceipt, CommandError> {
    match cmd {
        Command::InsertItem {
            scheme,
            position,
            item,
        } => insert_item(workspace, scheme, position, item),
        Command::UpdateItemText { scheme, item, text } => {
            update_item_text(workspace, scheme, item, text)
        }
        Command::ReplaceItem { scheme, item } => replace_item(workspace, scheme, item),
        Command::SetItemIndent {
            scheme,
            item,
            indent,
        } => set_item_indent(workspace, scheme, item, indent),
        Command::SetItemMarker {
            scheme,
            item,
            marker,
        } => set_item_marker(workspace, scheme, item, marker),
        Command::SetItemDate {
            scheme,
            item,
            kind,
            date,
        } => set_item_date(workspace, scheme, item, kind, date),
        Command::SetItemRecurrence {
            scheme,
            item,
            repeats,
        } => set_item_recurrence(workspace, scheme, item, repeats),
        Command::SetItemPriority {
            scheme,
            item,
            priority,
        } => set_item_priority(workspace, scheme, item, priority),
        Command::SetOccurrenceNotificationOffset {
            scheme,
            item,
            occurrence,
            offset_secs,
        } => set_occurrence_notification_offset(workspace, scheme, item, occurrence, offset_secs),
        Command::ToggleOccurrence {
            scheme,
            item,
            occurrence,
        } => toggle_occurrence(workspace, scheme, item, occurrence),
        Command::DeleteItem { scheme, item } => delete_item(workspace, scheme, item),
        Command::ReorderItem { scheme, from, to } => reorder_item(workspace, scheme, from, to),
        _ => unreachable!("non-item command dispatched to item handler"),
    }
}

fn insert_item(
    workspace: &mut Workspace,
    scheme: SchemeId,
    position: usize,
    mut item: Item,
) -> Result<CommandReceipt, CommandError> {
    let scheme_obj = workspace
        .schemes
        .get_mut(&scheme)
        .ok_or(CommandError::SchemeMissing(scheme))?;
    if position > scheme_obj.items.len() {
        return Err(CommandError::BadPosition(position));
    }
    item.enforce_marker_constraints();
    let id = item.id;
    scheme_obj.items.insert(position, item);
    Ok(CommandReceipt {
        inverse: Command::DeleteItem { scheme, item: id },
        touched: ChangeSet::default().touched_scheme(scheme),
    })
}

fn update_item_text(
    workspace: &mut Workspace,
    scheme: SchemeId,
    item: ItemId,
    text: String,
) -> Result<CommandReceipt, CommandError> {
    let item_ref = item_mut(workspace, scheme, item)?;
    let prev = item_ref.text();
    item_ref.set_text(text);
    Ok(CommandReceipt {
        inverse: Command::UpdateItemText {
            scheme,
            item,
            text: prev,
        },
        touched: ChangeSet::default().touched_scheme(scheme),
    })
}

fn replace_item(
    workspace: &mut Workspace,
    scheme: SchemeId,
    mut item: Item,
) -> Result<CommandReceipt, CommandError> {
    let scheme_obj = workspace
        .schemes
        .get_mut(&scheme)
        .ok_or(CommandError::SchemeMissing(scheme))?;
    let pos = scheme_obj
        .item_index(item.id)
        .ok_or(CommandError::ItemMissing(item.id, scheme))?;
    item.enforce_marker_constraints();
    let prev = std::mem::replace(&mut scheme_obj.items[pos], item);
    Ok(CommandReceipt {
        inverse: Command::ReplaceItem { scheme, item: prev },
        touched: ChangeSet::default().touched_scheme(scheme),
    })
}

fn set_item_indent(
    workspace: &mut Workspace,
    scheme: SchemeId,
    item: ItemId,
    indent: u8,
) -> Result<CommandReceipt, CommandError> {
    let item_ref = item_mut(workspace, scheme, item)?;
    let prev = item_ref.indent;
    item_ref.indent = indent;
    Ok(CommandReceipt {
        inverse: Command::SetItemIndent {
            scheme,
            item,
            indent: prev,
        },
        touched: ChangeSet::default().touched_scheme(scheme),
    })
}

fn set_item_marker(
    workspace: &mut Workspace,
    scheme: SchemeId,
    item: ItemId,
    marker: ItemMarker,
) -> Result<CommandReceipt, CommandError> {
    let item_ref = item_mut(workspace, scheme, item)?;
    let prev = item_ref.clone();
    item_ref.marker = marker;
    item_ref.enforce_marker_constraints();
    Ok(CommandReceipt {
        inverse: Command::ReplaceItem { scheme, item: prev },
        touched: ChangeSet::default().touched_scheme(scheme),
    })
}

fn set_item_date(
    workspace: &mut Workspace,
    scheme: SchemeId,
    item: ItemId,
    kind: DateKind,
    date: Option<chrono::DateTime<chrono::Utc>>,
) -> Result<CommandReceipt, CommandError> {
    let item_ref = item_mut(workspace, scheme, item)?;
    if date.is_some() {
        if let Some(prev) = ensure_checkbox(item_ref) {
            *date_slot(item_ref, kind) = date;
            return Ok(CommandReceipt {
                inverse: Command::ReplaceItem { scheme, item: prev },
                touched: ChangeSet::default().touched_scheme(scheme),
            });
        }
    }
    let prev = std::mem::replace(date_slot(item_ref, kind), date);
    Ok(CommandReceipt {
        inverse: Command::SetItemDate {
            scheme,
            item,
            kind,
            date: prev,
        },
        touched: ChangeSet::default().touched_scheme(scheme),
    })
}

fn set_item_recurrence(
    workspace: &mut Workspace,
    scheme: SchemeId,
    item: ItemId,
    repeats: Option<Recurrence>,
) -> Result<CommandReceipt, CommandError> {
    let item_ref = item_mut(workspace, scheme, item)?;
    if repeats.is_some() {
        if let Some(prev) = ensure_checkbox(item_ref) {
            item_ref.repeats = repeats;
            return Ok(CommandReceipt {
                inverse: Command::ReplaceItem { scheme, item: prev },
                touched: ChangeSet::default().touched_scheme(scheme),
            });
        }
    }
    let prev = std::mem::replace(&mut item_ref.repeats, repeats);
    Ok(CommandReceipt {
        inverse: Command::SetItemRecurrence {
            scheme,
            item,
            repeats: prev,
        },
        touched: ChangeSet::default().touched_scheme(scheme),
    })
}

fn set_item_priority(
    workspace: &mut Workspace,
    scheme: SchemeId,
    item: ItemId,
    priority: Option<u8>,
) -> Result<CommandReceipt, CommandError> {
    let item_ref = item_mut(workspace, scheme, item)?;
    let prev = item_ref.priority;
    item_ref.priority = priority;
    Ok(CommandReceipt {
        inverse: Command::SetItemPriority {
            scheme,
            item,
            priority: prev,
        },
        touched: ChangeSet::default().touched_scheme(scheme),
    })
}

fn set_occurrence_notification_offset(
    workspace: &mut Workspace,
    scheme: SchemeId,
    item: ItemId,
    occurrence: OccurrenceId,
    offset_secs: Option<i64>,
) -> Result<CommandReceipt, CommandError> {
    let item_ref = item_mut(workspace, scheme, item)?;
    let prev_item = ensure_checkbox(item_ref);
    let state = item_ref.state_for_occurrence_mut(occurrence.clone());
    let prev = std::mem::replace(&mut state.notification_offset_secs, offset_secs);
    item_ref.normalize_state();
    Ok(CommandReceipt {
        inverse: if let Some(item) = prev_item {
            Command::ReplaceItem { scheme, item }
        } else {
            Command::SetOccurrenceNotificationOffset {
                scheme,
                item,
                occurrence,
                offset_secs: prev,
            }
        },
        touched: ChangeSet::default().touched_scheme(scheme),
    })
}

fn toggle_occurrence(
    workspace: &mut Workspace,
    scheme: SchemeId,
    item: ItemId,
    occurrence: OccurrenceId,
) -> Result<CommandReceipt, CommandError> {
    let item_ref = item_mut(workspace, scheme, item)?;
    let prev = ensure_checkbox(item_ref);
    let state = item_ref.state_for_occurrence_mut(occurrence.clone());
    state.progress = if state.progress < 0 { 0 } else { -1 };
    Ok(CommandReceipt {
        inverse: if let Some(item) = prev {
            Command::ReplaceItem { scheme, item }
        } else {
            Command::ToggleOccurrence {
                scheme,
                item,
                occurrence,
            }
        },
        touched: ChangeSet::default().touched_scheme(scheme),
    })
}

fn delete_item(
    workspace: &mut Workspace,
    scheme: SchemeId,
    item: ItemId,
) -> Result<CommandReceipt, CommandError> {
    let scheme_obj = workspace
        .schemes
        .get_mut(&scheme)
        .ok_or(CommandError::SchemeMissing(scheme))?;
    let pos = scheme_obj
        .item_index(item)
        .ok_or(CommandError::ItemMissing(item, scheme))?;
    let removed = scheme_obj.items.remove(pos);
    Ok(CommandReceipt {
        inverse: Command::InsertItem {
            scheme,
            position: pos,
            item: removed,
        },
        touched: ChangeSet::default().touched_scheme(scheme),
    })
}

fn reorder_item(
    workspace: &mut Workspace,
    scheme: SchemeId,
    from: usize,
    to: usize,
) -> Result<CommandReceipt, CommandError> {
    let scheme_obj = workspace
        .schemes
        .get_mut(&scheme)
        .ok_or(CommandError::SchemeMissing(scheme))?;
    if from >= scheme_obj.items.len() || to >= scheme_obj.items.len() {
        return Err(CommandError::BadPosition(from.max(to)));
    }
    let moved = scheme_obj.items.remove(from);
    scheme_obj.items.insert(to, moved);
    Ok(CommandReceipt {
        inverse: Command::ReorderItem {
            scheme,
            from: to,
            to: from,
        },
        touched: ChangeSet::default().touched_scheme(scheme),
    })
}

/// If the item is not already a Checkbox, promotes it and returns the original.
fn ensure_checkbox(item: &mut Item) -> Option<Item> {
    if item.marker != ItemMarker::Checkbox {
        let prev = item.clone();
        item.marker = ItemMarker::Checkbox;
        item.enforce_marker_constraints();
        Some(prev)
    } else {
        None
    }
}

fn item_mut(
    workspace: &mut Workspace,
    scheme: SchemeId,
    item: ItemId,
) -> Result<&mut Item, CommandError> {
    workspace
        .schemes
        .get_mut(&scheme)
        .ok_or(CommandError::SchemeMissing(scheme))?
        .item_mut(item)
        .ok_or(CommandError::ItemMissing(item, scheme))
}

fn date_slot(item: &mut Item, kind: DateKind) -> &mut Option<chrono::DateTime<chrono::Utc>> {
    match kind {
        DateKind::Start => &mut item.start,
        DateKind::End => &mut item.end,
        DateKind::Available => &mut item.available,
    }
}
