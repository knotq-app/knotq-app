use chrono::{TimeZone, Utc};
use knotq_model::{Item, ItemKind, ItemMarker};

#[test]
fn item_kind_is_derived_from_marker_and_dates() {
    let dt = Utc.with_ymd_and_hms(2026, 1, 1, 9, 0, 0).unwrap();
    assert_eq!(
        Item::new("event").with_start(dt).with_end(dt).kind(),
        ItemKind::Event
    );
    assert_eq!(
        Item::new("assignment").with_end(dt).kind(),
        ItemKind::Assignment
    );
    assert_eq!(
        Item::new("reminder").with_start(dt).kind(),
        ItemKind::Reminder
    );
    assert_eq!(Item::new("procedure").kind(), ItemKind::Procedure);
    assert_eq!(
        Item::new("bullet")
            .with_start(dt)
            .with_marker(ItemMarker::Bullet)
            .kind(),
        ItemKind::Procedure
    );
}
