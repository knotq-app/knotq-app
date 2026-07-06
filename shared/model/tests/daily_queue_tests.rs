use chrono::NaiveDate;
use knotq_model::{
    daily_queue_displaced_item_id, daily_queue_document_id, daily_queue_scheme_id,
    daily_queue_sync_metadata, Item, ItemId, Scheme, Workspace, DAILY_QUEUE_COLOR_INDEX,
};

#[test]
fn daily_queue_ids_are_stable_by_date() {
    let date = NaiveDate::from_ymd_opt(2026, 5, 30).unwrap();
    let next = NaiveDate::from_ymd_opt(2026, 5, 31).unwrap();

    assert_eq!(daily_queue_scheme_id(date), daily_queue_scheme_id(date));
    assert_eq!(daily_queue_document_id(date), daily_queue_document_id(date));
    assert_ne!(daily_queue_scheme_id(date), daily_queue_scheme_id(next));
    assert_ne!(daily_queue_document_id(date), daily_queue_document_id(next));
}

#[test]
fn displaced_item_ids_are_deterministic_and_date_scoped() {
    let date = NaiveDate::from_ymd_opt(2026, 5, 30).unwrap();
    let next = NaiveDate::from_ymd_opt(2026, 5, 31).unwrap();
    let source = ItemId::new();
    let other = ItemId::new();

    // Determinism is what makes concurrent double-rolls converge: two devices
    // archiving the same row must mint the SAME id.
    assert_eq!(
        daily_queue_displaced_item_id(source, date),
        daily_queue_displaced_item_id(source, date)
    );
    // The archived id must never alias the live (carried) id, and a row rolled
    // forward day after day needs a distinct archived id each day.
    assert_ne!(daily_queue_displaced_item_id(source, date), source);
    assert_ne!(
        daily_queue_displaced_item_id(source, date),
        daily_queue_displaced_item_id(source, next)
    );
    assert_ne!(
        daily_queue_displaced_item_id(source, date),
        daily_queue_displaced_item_id(other, date)
    );
    // Displacing an already-displaced row (re-roll of an archive) still moves.
    let displaced = daily_queue_displaced_item_id(source, date);
    assert_ne!(daily_queue_displaced_item_id(displaced, next), displaced);
}

#[test]
fn sync_metadata_uses_stable_daily_document_id() {
    let date = NaiveDate::from_ymd_opt(2026, 5, 30).unwrap();
    let mut workspace = Workspace::new();
    let mut daily = Scheme::new("Daily 2026-05-30", DAILY_QUEUE_COLOR_INDEX);
    daily.id = daily_queue_scheme_id(date);
    daily.items.push(Item::new(""));

    workspace.daily_queue.insert(date, daily.id);
    workspace.schemes.insert(daily.id, daily);

    assert!(workspace.ensure_sync_metadata());
    assert_eq!(
        workspace.scheme_sync[&daily_queue_scheme_id(date)].id,
        daily_queue_sync_metadata(date).id
    );
}
