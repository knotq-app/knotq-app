use knotq_commands::{Command, DateKind};
use knotq_model::{Item, ItemMarker, Scheme, SchemeId};

use super::ImportedEvent;

pub fn map_to_commands(
    events: Vec<ImportedEvent>,
    scheme_id: SchemeId,
    _existing: Option<&Scheme>,
) -> Vec<Command> {
    events
        .into_iter()
        .enumerate()
        .map(|(position, event)| Command::InsertItem {
            scheme: scheme_id,
            position,
            item: item_from_imported_event(event),
        })
        .collect()
}

pub fn event_update_commands(
    event: ImportedEvent,
    scheme_id: SchemeId,
    item: &Item,
) -> Vec<Command> {
    let mut commands = Vec::new();
    if item.text != event.summary {
        commands.push(Command::UpdateItemText {
            scheme: scheme_id,
            item: item.id,
            text: event.summary,
        });
    }
    if item.start != event.start {
        commands.push(Command::SetItemDate {
            scheme: scheme_id,
            item: item.id,
            kind: DateKind::Start,
            date: event.start,
        });
    }
    if item.end != event.end {
        commands.push(Command::SetItemDate {
            scheme: scheme_id,
            item: item.id,
            kind: DateKind::End,
            date: event.end,
        });
    }
    if item.repeats != event.recurrence {
        commands.push(Command::SetItemRecurrence {
            scheme: scheme_id,
            item: item.id,
            repeats: event.recurrence,
        });
    }
    commands
}

fn item_from_imported_event(event: ImportedEvent) -> Item {
    let mut item = Item::new(event.summary);
    item.start = event.start;
    item.end = event.end;
    item.repeats = event.recurrence;
    if item.start.is_some() || item.end.is_some() || item.repeats.is_some() {
        item.marker = ItemMarker::Checkbox;
    }
    item
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::*;

    #[test]
    fn imported_event_with_dates_maps_to_checkbox_item() {
        let scheme_id = SchemeId::new();
        let start = Utc.with_ymd_and_hms(2026, 1, 5, 11, 30, 0).unwrap();
        let event = ImportedEvent {
            uid: "1".to_string(),
            summary: "MATH 15".to_string(),
            start: Some(start),
            end: None,
            recurrence: None,
        };

        let commands = map_to_commands(vec![event], scheme_id, None);

        let Command::InsertItem { item, .. } = &commands[0] else {
            panic!("expected insert command");
        };
        assert_eq!(item.text, "MATH 15");
        assert_eq!(item.marker, ItemMarker::Checkbox);
        assert_eq!(item.start, Some(start));
    }
}
