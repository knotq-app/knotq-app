//! A Daily Queue page outside the loaded date window must keep its entry in the
//! workspace index when the device writes the index.
use super::super::*;

use chrono::Duration;
use knotq_model::{daily_queue_scheme_id, daily_queue_scheme_name, Item, NodeRef};

/// Production-fuzz seed 4: after a relaunch the desktop loads Daily pages only
/// up to today, so tomorrow's page is absent from the loaded workspace while its
/// `daily_queue` and `scheme_sync` bindings remain. The next local index edit
/// (a gsync toggle on another scheme) rewrote the index from the loaded schemes
/// and deleted the page's entry, so every device lost the day — its lines and
/// bindings intact but no longer materializable.
#[test]
fn an_unloaded_daily_page_keeps_its_index_entry_when_the_index_is_written() {
    let today = chrono::NaiveDate::from_ymd_opt(2026, 9, 15).unwrap();
    let tomorrow = today + Duration::days(1);

    let mut full = Workspace::new();
    let mut plans = Scheme::new("Plans", 0);
    plans.items.push(Item::new("line"));
    let plans_id = plans.id;
    full.schemes.insert(plans_id, plans);
    let root = full.root;
    full.folders
        .get_mut(&root)
        .unwrap()
        .children
        .push(NodeRef::Scheme(plans_id));
    let day_id = daily_queue_scheme_id(tomorrow);
    let mut day = Scheme::new(daily_queue_scheme_name(tomorrow), 0);
    day.id = day_id;
    day.items.push(Item::new("written on another device"));
    full.schemes.insert(day_id, day);
    full.daily_queue.insert(tomorrow, day_id);
    full.ensure_sync_metadata();
    let mut docs = WorkspaceCrdtDocuments::try_new(&full).unwrap();

    // Relaunched: tomorrow's page is outside the loaded window, its bindings kept.
    let mut loaded = full.clone();
    loaded.schemes.remove(&day_id);
    assert!(loaded.daily_queue.contains_key(&tomorrow));
    assert!(loaded.scheme_sync.contains_key(&day_id));
    loaded.schemes.get_mut(&plans_id).unwrap().gsync = true;

    let outcome = docs.sync_changes(&loaded, &WorkspaceCrdtChangeSet::default().workspace());
    assert!(outcome.is_ok(), "{:?}", outcome.errors);

    let materialized = docs
        .materialized_workspace_repair(&full, &|_| false)
        .unwrap();
    assert!(
        materialized.schemes.contains_key(&day_id),
        "tomorrow's page lost its index entry when a device without it loaded wrote the index"
    );
    assert!(
        materialized.schemes[&plans_id].gsync,
        "the index edit itself landed"
    );
}
