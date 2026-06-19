//! Regression tests for rich sync payloads that are easy to under-cover with
//! text-only convergence checks.

mod common;

use chrono::{Duration, NaiveDate, TimeZone, Utc};
use common::rich_items::{
    image_name as rich_image_name, image_ref, patterned_bytes, replace_scheme_items,
};
use common::{DeviceKey, Harness, D0, D1, D2};
use knotq_model::{
    CalendarDateTime, CalendarProvider, CalendarRecurrence, ExternalItemSource, ImageInline, Item,
    ItemMarker, ItemState, NodeRef, OccurrenceId, OccurrenceOverride, OccurrenceOverrideStatus,
    OccurrenceState, RawCalendarPayload, Table, TableCell,
};
use knotq_notifications::{
    compute_due_notifications_with_lead_times, notification_keys_for_item, NotificationLeadTimes,
};
use knotq_sync::WorkspaceCrdtChangeSet;

#[test]
fn rich_daily_queue_payload_and_nested_media_survive_late_peer_sync() {
    let mut h = Harness::new(3);
    h.login_all();

    let day = NaiveDate::from_ymd_opt(2027, 3, 14).unwrap();
    let top_image = image_ref(320, 180);
    let cell_image = image_ref(96, 54);
    let top_image_name = rich_image_name(top_image);
    let cell_image_name = rich_image_name(cell_image);
    let top_image_bytes = patterned_bytes(3072, 251);
    let cell_image_bytes = patterned_bytes(1536, 127);

    let daily = h.seed_daily_queue(
        D0,
        day,
        vec![
            rich_markdown_task(),
            top_level_image_item(top_image),
            table_item_with_nested_payload(cell_image),
        ],
    );
    h.device_mut_for_surgery(D0)
        .media_assets
        .insert(top_image_name.clone(), top_image_bytes.clone());
    h.device_mut_for_surgery(D0)
        .media_assets
        .insert(cell_image_name.clone(), cell_image_bytes.clone());
    let expected_items = h.device(D0).workspace.schemes[&daily].items.clone();

    h.sync(D0);
    let latest = h.device_remote_latest(D0);
    h.upload_media(D0, &latest).expect("upload daily media");

    for peer in [D1, D2] {
        h.device_mut_for_surgery(peer).restart();
        h.sync(peer);
        h.download_media(peer);
    }
    h.sync(D0);
    h.settle();

    for key in h.device_keys() {
        h.download_media(key);
        assert_eq!(
            h.device(key).workspace.daily_queue_scheme_id(day),
            Some(daily),
            "{key:?}: daily queue mapping changed"
        );
        assert_eq!(
            h.device(key).workspace.schemes[&daily].items,
            expected_items,
            "{key:?}: rich daily payload did not round-trip exactly"
        );
        assert_eq!(
            h.device(key).media_assets.get(&top_image_name),
            Some(&top_image_bytes),
            "{key:?}: top-level image bytes missing or corrupted"
        );
        assert_eq!(
            h.device(key).media_assets.get(&cell_image_name),
            Some(&cell_image_bytes),
            "{key:?}: table-cell image bytes missing or corrupted"
        );
    }
    h.assert_all_converged();
}

#[test]
fn archived_nested_folder_keeps_rich_child_scheme_after_concurrent_edit() {
    let mut h = Harness::new(2);
    h.login_all();

    let project = h.add_folder(D0, "Project Archive");
    let specs = h.add_subfolder(D0, project, "Specs");
    let scheme = h.add_scheme_to_folder(D0, specs, "Spec Sheet", &["placeholder"]);
    h.settle();

    replace_scheme_items(
        &mut h,
        D0,
        scheme,
        vec![
            rich_markdown_task(),
            table_item_with_nested_payload(image_ref(48, 48)),
        ],
    );
    h.settle();

    h.archive_folder(D0, project);
    {
        let device = h.device_mut_for_surgery(D1);
        let items = &mut device.scheme_mut_pub(scheme).items;
        items[0].set_text("**Spec** edited while the folder is archived");
        let table = items[1].table_mut().expect("table item");
        table.rows[0].cells[0].items[0].priority = Some(1);
        table.rows[1].cells[1]
            .items
            .push(Item::new("peer note").with_indent(2));
        device.record_changes(WorkspaceCrdtChangeSet::default().touch_scheme(scheme));
    }

    h.sync(D1);
    h.sync(D0);
    h.sync(D1);
    h.settle();
    h.assert_all_converged();

    for key in h.device_keys() {
        let workspace = &h.device(key).workspace;
        assert!(
            workspace.is_folder_deleted(project),
            "{key:?}: folder not archived"
        );
        assert!(
            workspace.folders[&project]
                .children
                .contains(&NodeRef::Folder(specs)),
            "{key:?}: archived parent lost nested folder"
        );
        assert!(
            workspace.folders[&specs]
                .children
                .contains(&NodeRef::Scheme(scheme)),
            "{key:?}: nested folder lost child scheme"
        );
        assert!(
            workspace.is_scheme_deleted(scheme),
            "{key:?}: child scheme should remain archived with its folder"
        );

        let items = &workspace.schemes[&scheme].items;
        assert_eq!(
            items[0].text(),
            "**Spec** edited while the folder is archived"
        );
        let table = items[1].table().expect("table item");
        assert_eq!(table.rows[0].cells[0].items[0].priority, Some(1));
        assert_eq!(table.rows[1].cells[1].items[1].text(), "peer note");
        assert_eq!(table.rows[1].cells[1].items[1].indent, 2);
    }
}

#[test]
fn synced_completion_cancels_due_notification_after_concurrent_content_edit() {
    let mut h = Harness::new(2);
    h.login_all();

    let trigger = Utc.with_ymd_and_hms(2027, 4, 2, 9, 0, 0).unwrap();
    let scheme = h.add_scheme(D0, "Notifications", &["placeholder"]);
    replace_scheme_items(&mut h, D0, scheme, vec![notification_task(trigger)]);
    h.settle();

    let item_id = h.device(D0).workspace.schemes[&scheme].items[0].id;
    let lead_times = NotificationLeadTimes {
        reminder_offset_secs: 0,
        event_offset_secs: 0,
        assignment_offset_secs: 0,
    };
    for key in h.device_keys() {
        assert_eq!(
            due_notifications(&h, key, lead_times, trigger).len(),
            1,
            "{key:?}: reminder should be schedulable before completion sync"
        );
    }

    h.edit_line(D0, scheme, 0, "file amended tax return");
    {
        let device = h.device_mut_for_surgery(D1);
        let item = &mut device.scheme_mut_pub(scheme).items[0];
        item.state[0].state.progress = -1;
        item.state[0].state.notification_offset_secs = None;
        device.record_changes(WorkspaceCrdtChangeSet::default().touch_scheme(scheme));
    }

    h.sync(D1);
    h.sync(D0);
    h.sync(D1);
    h.settle();
    h.assert_all_converged();

    for key in h.device_keys() {
        let item = &h.device(key).workspace.schemes[&scheme].items[0];
        assert_eq!(item.text(), "file amended tax return");
        assert!(
            item.state[0].state.is_done(),
            "{key:?}: completion did not sync"
        );
        assert!(
            due_notifications(&h, key, lead_times, trigger).is_empty(),
            "{key:?}: completed reminder should not schedule after sync"
        );
        let cleanup_keys = notification_keys_for_item(
            &h.device(key).workspace,
            lead_times,
            scheme,
            item_id,
            trigger - Duration::minutes(1),
            trigger + Duration::minutes(1),
        );
        assert_eq!(
            cleanup_keys.len(),
            1,
            "{key:?}: stale OS notification key should remain discoverable for cancellation"
        );
    }
}

fn rich_markdown_task() -> Item {
    let start = Utc.with_ymd_and_hms(2027, 3, 14, 10, 30, 0).unwrap();
    let end = start + Duration::hours(2);
    let available = start - Duration::days(1);
    let recurring = OccurrenceId::recurring_utc(start + Duration::weeks(1));
    let mut item = Item::new("**Launch** _notes_ `code` | table-ish text");
    item.marker = ItemMarker::Checkbox;
    item.indent = 2;
    item.start = Some(start);
    item.end = Some(end);
    item.available = Some(available);
    item.repeats = Some(CalendarRecurrence {
        rrules: vec!["FREQ=WEEKLY;COUNT=3".to_string()],
        rdates: vec![CalendarDateTime::utc(start + Duration::days(2))],
        exdates: vec![CalendarDateTime::utc(start + Duration::weeks(2))],
        overrides: vec![OccurrenceOverride {
            occurrence: recurring.clone(),
            status: OccurrenceOverrideStatus::Cancelled,
            start: None,
            end: None,
            available: None,
        }],
        raw_import: Some(RawCalendarPayload {
            content_type: "text/calendar".to_string(),
            data: "BEGIN:VEVENT\nSUMMARY:Launch\nEND:VEVENT".to_string(),
        }),
    });
    item.state[0].state.notification_offset_secs = Some(15 * 60);
    item.state.push(OccurrenceState {
        occurrence: recurring,
        state: ItemState {
            progress: -1,
            notification_offset_secs: Some(-30 * 60),
        },
    });
    item.priority = Some(2);
    item.external = Some(ExternalItemSource {
        provider: CalendarProvider::Google,
        account_id: "acct-123".to_string(),
        calendar_id: "primary".to_string(),
        event_id: "event-456".to_string(),
        instance_id: Some("instance-789".to_string()),
        updated_at: Some(start - Duration::minutes(5)),
    });
    item
}

fn top_level_image_item(image: ImageInline) -> Item {
    let mut item = Item::new("");
    item.indent = 1;
    item.set_image(image);
    item
}

fn table_item_with_nested_payload(image: ImageInline) -> Item {
    let mut table = Table::new(2, 2);
    table.columns[0].name = "Task".to_string();
    table.columns[0].width = Some(180.0);
    table.columns[1].name = "Evidence".to_string();
    table.columns[1].width = Some(220.0);
    table.rows[0].cells[0] = TableCell::from_items(vec![
        Item::new("cell task").with_marker(ItemMarker::Checkbox),
        Item::new("cell subnote").with_indent(1),
    ]);
    let mut image_item = Item::new("");
    image_item.set_image(image);
    table.rows[0].cells[1] = TableCell::from_items(vec![Item::new("diagram"), image_item]);
    table.rows[1].cells[0] = TableCell::with_text("r2 c1");
    table.rows[1].cells[1] = TableCell::from_items(vec![Item::new("r2 c2").with_indent(1)]);

    let mut item = Item::new("");
    item.marker = ItemMarker::Bullet;
    item.set_table(table);
    item
}

fn notification_task(trigger: chrono::DateTime<Utc>) -> Item {
    let mut item = Item::new("file tax return").with_start(trigger);
    item.state[0].state.notification_offset_secs = Some(0);
    item
}

fn due_notifications(
    h: &Harness,
    key: DeviceKey,
    lead_times: NotificationLeadTimes,
    trigger: chrono::DateTime<Utc>,
) -> Vec<knotq_notifications::ScheduledNotification> {
    compute_due_notifications_with_lead_times(
        &h.device(key).workspace,
        lead_times,
        trigger - Duration::minutes(1),
        trigger + Duration::minutes(1),
    )
}
