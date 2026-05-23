use chrono::{Duration, TimeZone, Utc};
use knotq_index::IndexedWorkspace;
use knotq_model::{Item, NodeRef, Scheme, Workspace};

#[test]
fn calendar_query_returns_occurrences_with_origin_context() {
    let start = Utc.with_ymd_and_hms(2026, 1, 5, 10, 0, 0).unwrap();
    let end = Utc.with_ymd_and_hms(2026, 1, 5, 11, 0, 0).unwrap();
    let (workspace, scheme_id, item_id) =
        workspace_with_item(Item::new("Class").with_start(start).with_end(end));
    let indexed = IndexedWorkspace::build(workspace);

    let events = indexed.calendar_query().upcoming(start, 10);

    assert_eq!(events.len(), 1);
    assert_eq!(events[0].scheme_id, scheme_id);
    assert_eq!(events[0].item_id, item_id);
    assert_eq!(events[0].color_index, 2);
}

#[test]
fn overdue_query_skips_assignments_older_than_seven_days() {
    let as_of = Utc.with_ymd_and_hms(2026, 1, 10, 12, 0, 0).unwrap();
    let due = as_of - Duration::days(8);
    let (workspace, _, _) = workspace_with_item(Item::new("Old essay").with_end(due));
    let indexed = IndexedWorkspace::build(workspace);

    let events = indexed.calendar_query().overdue(as_of);

    assert!(events.is_empty());
}

fn workspace_with_item(item: Item) -> (Workspace, knotq_model::SchemeId, knotq_model::ItemId) {
    let item_id = item.id;
    let mut workspace = Workspace::new();
    let mut scheme = Scheme::new("Classes", 2);
    scheme.items.push(item);
    let scheme_id = scheme.id;
    workspace.schemes.insert(scheme_id, scheme);
    workspace
        .folders
        .get_mut(&workspace.root)
        .unwrap()
        .children
        .push(NodeRef::Scheme(scheme_id));
    (workspace, scheme_id, item_id)
}
