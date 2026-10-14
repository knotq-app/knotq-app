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
use sha2::{Digest, Sha256};

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
fn media_recovery_reuploads_local_bytes_after_stale_uploaded_cursor() {
    let mut h = Harness::new(2);
    h.login_all();

    let scheme = h.add_scheme(D0, "Recovered Media", &["photo"]);
    h.settle();

    let image_bytes = patterned_bytes(1280, 251);
    let (_, image_name) = h.attach_image_to_device(D0, scheme, 0, image_bytes.clone());
    h.sync(D0);
    let document = h.device(D0).workspace.scheme_sync[&scheme].id;
    let remote_latest = std::collections::HashMap::from([(document, 1)]);
    let sha256 = sha256_hex(&image_bytes);
    h.device_mut_for_surgery(D0)
        .local_state_mut()
        .mark_media_uploaded(
            image_name.clone(),
            document,
            image_bytes.len() as u64,
            sha256,
        );

    h.upload_media(D0, &remote_latest)
        .expect("pre-recovery upload");
    assert_eq!(
        h.server_media_asset_count(),
        0,
        "stale upload cursor should suppress upload before recovery"
    );

    h.device_mut_for_surgery(D0)
        .local_state_mut()
        .heal_for_recovery_version();
    h.upload_media(D0, &remote_latest).expect("recovery upload");
    assert_eq!(h.server_media_asset_count(), 1);

    h.sync(D1);
    h.download_media(D1);
    assert_eq!(
        h.device(D1).media_assets.get(&image_name),
        Some(&image_bytes),
        "peer should download image bytes re-uploaded by recovery"
    );
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

/// Resizing an image rewrites the size stored on its line and nothing else: the
/// asset, and so the uploaded bytes, stay as they were.
#[test]
fn resized_image_reaches_a_peer_with_its_new_size() {
    let mut h = Harness::new(2);
    h.login_all();

    let scheme = h.add_scheme(D0, "Screenshots", &["placeholder"]);
    h.settle();
    let image = image_ref(1200, 800);
    let line = top_level_image_item(image);
    replace_scheme_items(&mut h, D0, scheme, vec![Item::new("caption"), line.clone()]);
    h.settle();

    resize_image_line(&mut h, D1, scheme, 1, 300, 200);
    h.settle();
    h.assert_all_converged();

    for key in h.device_keys() {
        let items = &h.device(key).workspace.schemes[&scheme].items;
        assert_eq!(items.len(), 2, "{key:?}: a resize changed the line count");
        assert_eq!(items[1].id, line.id, "{key:?}: a resize replaced the line");
        assert_eq!(
            items[1].content.image(),
            Some(&ImageInline {
                width: Some(300),
                height: Some(200),
                ..image
            }),
            "{key:?}: the resized image did not arrive intact"
        );
    }
}

/// Two devices dragging the same image at once must still leave one image on
/// the line, at one of the two sizes, on every device.
#[test]
fn concurrent_image_resizes_converge_on_one_image() {
    let mut h = Harness::new(3);
    h.login_all();

    let scheme = h.add_scheme(D0, "Screenshots", &["placeholder"]);
    h.settle();
    let image = image_ref(1200, 800);
    let line = top_level_image_item(image);
    replace_scheme_items(&mut h, D0, scheme, vec![line.clone(), Item::new("after")]);
    h.settle();

    resize_image_line(&mut h, D0, scheme, 0, 600, 400);
    resize_image_line(&mut h, D1, scheme, 0, 240, 160);
    h.sync(D0);
    h.sync(D1);
    h.sync(D0);
    h.settle();
    h.assert_all_converged();

    let winner = h.device(D2).workspace.schemes[&scheme].items[0]
        .content
        .image()
        .copied()
        .expect("the line is still an image");
    assert_eq!(winner.asset, image.asset, "the resize changed the asset");
    assert!(
        [(Some(600), Some(400)), (Some(240), Some(160))].contains(&(winner.width, winner.height)),
        "converged on a size neither device chose: {winner:?}"
    );
    for key in h.device_keys() {
        let items = &h.device(key).workspace.schemes[&scheme].items;
        assert_eq!(items.len(), 2, "{key:?}: a resize changed the line count");
        assert_eq!(items[0].id, line.id, "{key:?}: a resize replaced the line");
        assert_eq!(items[0].content.image(), Some(&winner), "{key:?}");
        assert_eq!(items[1].text(), "after", "{key:?}");
    }

    // The line must stay editable afterwards: a later resize is one more plain
    // edit, not a second image.
    resize_image_line(&mut h, D2, scheme, 0, 90, 60);
    h.settle();
    h.assert_all_converged();
    for key in h.device_keys() {
        let items = &h.device(key).workspace.schemes[&scheme].items;
        assert_eq!(items.len(), 2, "{key:?}");
        let resized = items[0].content.image().expect("still an image");
        assert_eq!(
            (resized.width, resized.height),
            (Some(90), Some(60)),
            "{key:?}"
        );
    }
}

fn resize_image_line(
    h: &mut Harness,
    key: DeviceKey,
    scheme: knotq_model::SchemeId,
    index: usize,
    width: u32,
    height: u32,
) {
    let device = h.device_mut_for_surgery(key);
    let item = &mut device.scheme_mut_pub(scheme).items[index];
    let image = *item.content.image().expect("an image line");
    item.set_image(ImageInline {
        width: Some(width),
        height: Some(height),
        ..image
    });
    device.record_changes(WorkspaceCrdtChangeSet::default().touch_scheme(scheme));
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
    item.repeats = Some(Box::new(CalendarRecurrence {
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
    }));
    item.state[0].state.notification_offset_secs = Some(15 * 60);
    item.state.push(OccurrenceState {
        occurrence: recurring,
        state: ItemState {
            progress: -1,
            notification_offset_secs: Some(-30 * 60),
        },
    });
    item.priority = Some(2);
    item.external = Some(Box::new(ExternalItemSource {
        provider: CalendarProvider::Google,
        account_id: "acct-123".to_string(),
        calendar_id: "primary".to_string(),
        event_id: "event-456".to_string(),
        instance_id: Some("instance-789".to_string()),
        updated_at: Some(start - Duration::minutes(5)),
    }));
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

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}
