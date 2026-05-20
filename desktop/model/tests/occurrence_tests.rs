use chrono::{TimeZone, Utc};
use knotq_model::{Item, ItemState, OccurrenceId};

#[test]
fn state_for_occurrence_prefers_recurring_override() {
    let start = Utc.with_ymd_and_hms(2026, 1, 1, 9, 0, 0).unwrap();
    let occurrence = OccurrenceId::recurring_utc(start);
    let mut item = Item::new("repeat");
    item.state_for_occurrence_mut(OccurrenceId::Single)
        .notification_offset_secs = Some(60);
    *item.state_for_occurrence_mut(occurrence.clone()) = ItemState {
        progress: -1,
        notification_offset_secs: None,
    };
    assert!(item.state_for_occurrence(&occurrence).is_done());
    assert!(!item.state_for_occurrence(&OccurrenceId::Single).is_done());
}
