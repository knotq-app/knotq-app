use chrono::NaiveDate;
use knotq_model::{
    daily_queue_document_id, daily_queue_scheme_id, daily_queue_sync_metadata, Item, Scheme,
    Workspace, DAILY_QUEUE_COLOR_INDEX,
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
