use chrono::{Local, NaiveDate, Timelike};
use knotq_commands::Command;
use knotq_model::{daily_queue_scheme_id, DocumentId, Item, NodeRef, Scheme, SchemeId, Workspace};
use knotq_state::{
    daily_queue_carryover_command, daily_queue_scheme_is_blank, last_nonempty_daily_queue_day,
    make_default_workspace_for_date, DailyQueueState,
};

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

/// Insert a daily-queue scheme for `day` carrying `items` (text only).
fn insert_daily(workspace: &mut Workspace, day: NaiveDate, items: &[&str]) {
    let id = daily_queue_scheme_id(day);
    let mut scheme = Scheme::new(format!("Daily {day}"), 0);
    scheme.id = id;
    scheme.items = items.iter().map(|text| Item::new(*text)).collect();
    workspace.daily_queue.insert(day, id);
    workspace.schemes.insert(id, scheme);
}

#[test]
fn blank_daily_queue_scheme_is_detected() {
    let scheme = Scheme::new("Daily", 0);
    assert!(daily_queue_scheme_is_blank(&scheme));

    let mut scheme = Scheme::new("Daily", 0);
    scheme.items.push(Item::new(""));
    assert!(daily_queue_scheme_is_blank(&scheme));
}

#[test]
fn carryover_moves_incomplete_items_to_today() {
    let mut previous = Scheme::new("Yesterday", 0);
    previous.items.push(Item::new("Finish draft"));
    let mut today = Scheme::new("Today", 0);
    today.items.push(Item::new(""));

    let command = daily_queue_carryover_command(previous.id, &previous, today.id, &today);

    assert!(matches!(command, Some(Command::Batch(_))));
}

#[test]
fn carryover_inserts_into_empty_today() {
    let mut previous = Scheme::new("Yesterday", 0);
    previous.items.push(Item::new("Finish draft"));
    let today = Scheme::new("Today", 0);

    let command = daily_queue_carryover_command(previous.id, &previous, today.id, &today);

    let Some(Command::Batch(commands)) = command else {
        panic!("expected carryover batch");
    };
    assert!(matches!(
        commands.as_slice(),
        [Command::InsertItem { position: 0, .. }]
    ));
}

#[test]
fn last_nonempty_day_prefers_yesterday() {
    let today = date(2026, 6, 16);
    let mut workspace = Workspace::new();
    insert_daily(&mut workspace, date(2026, 6, 15), &["Finish draft"]);
    insert_daily(&mut workspace, date(2026, 6, 10), &["Older item"]);

    assert_eq!(
        last_nonempty_daily_queue_day(&workspace, today),
        Some(date(2026, 6, 15))
    );
}

#[test]
fn last_nonempty_day_skips_blank_days_within_window() {
    let today = date(2026, 6, 16);
    let mut workspace = Workspace::new();
    // Yesterday exists but is blank (single empty placeholder row).
    insert_daily(&mut workspace, date(2026, 6, 15), &[""]);
    insert_daily(&mut workspace, date(2026, 6, 9), &["Plan the week"]);

    assert_eq!(
        last_nonempty_daily_queue_day(&workspace, today),
        Some(date(2026, 6, 9))
    );
}

#[test]
fn last_nonempty_day_ignores_content_older_than_two_weeks() {
    let today = date(2026, 6, 16);
    let mut workspace = Workspace::new();
    // 15 days back is just outside the two-week lookback (offsets 1..=14).
    insert_daily(&mut workspace, date(2026, 6, 1), &["Too old to roll over"]);

    assert_eq!(last_nonempty_daily_queue_day(&workspace, today), None);
}

#[test]
fn day_boundary_sync_updates_today_once() {
    let today = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
    let tomorrow = NaiveDate::from_ymd_opt(2026, 1, 2).unwrap();
    let mut state = DailyQueueState::new(today, today);

    assert!(state.sync_day_boundary(tomorrow));
    assert!(!state.sync_day_boundary(tomorrow));
}

#[test]
fn default_workspace_uses_fixed_starter_ids_and_plain_calendar_titles() {
    let workspace = make_default_workspace_for_date(date(2026, 6, 18));
    let root = workspace.folder(workspace.root).unwrap();
    let starter_ids = [
        "00000000-0000-8000-8000-000000000101"
            .parse::<SchemeId>()
            .unwrap(),
        "00000000-0000-8000-8000-000000000102"
            .parse::<SchemeId>()
            .unwrap(),
        "00000000-0000-8000-8000-000000000103"
            .parse::<SchemeId>()
            .unwrap(),
    ];
    let starter_documents = [
        "00000000-0000-8000-8000-000000000201"
            .parse::<DocumentId>()
            .unwrap(),
        "00000000-0000-8000-8000-000000000202"
            .parse::<DocumentId>()
            .unwrap(),
        "00000000-0000-8000-8000-000000000203"
            .parse::<DocumentId>()
            .unwrap(),
    ];

    for (scheme_id, document_id) in starter_ids.into_iter().zip(starter_documents) {
        assert!(root.children.contains(&NodeRef::Scheme(scheme_id)));
        assert_eq!(workspace.scheme_sync[&scheme_id].id, document_id);
    }

    let mut highlight_count = 0;
    let mut saw_work_session = false;
    for scheme in workspace.iter_schemes() {
        for item in &scheme.items {
            let text = item.text();
            if text == "Work session" {
                let end = item.end.unwrap().with_timezone(&Local);
                assert_eq!((end.hour(), end.minute()), (15, 0));
                saw_work_session = true;
            }
            assert!(
                !text.contains("~~"),
                "starter workspace should not include strikethrough examples: {text}"
            );
            if text.contains("==") {
                highlight_count += 1;
            }
            if text.trim_start().starts_with('#') {
                assert!(text.trim_start().starts_with("### "));
            }
            if item.start.is_some() || item.end.is_some() || item.repeats.is_some() {
                assert!(text.len() <= 18, "calendar title is too long: {text}");
                assert!(
                    !["#", "**", "*", "==", "~~"]
                        .iter()
                        .any(|marker| text.contains(marker)),
                    "calendar title should be plain text: {text}"
                );
            }
        }
    }
    assert_eq!(highlight_count, 2);
    assert!(saw_work_session);

    let scheduling = workspace
        .scheme(
            "00000000-0000-8000-8000-000000000102"
                .parse::<SchemeId>()
                .unwrap(),
        )
        .unwrap();
    let morning_review = scheduling
        .items
        .iter()
        .find(|item| item.text() == "Morning review")
        .unwrap();
    assert_eq!(
        morning_review.repeats.as_ref().unwrap().rrules,
        vec!["FREQ=WEEKLY;INTERVAL=1"]
    );
}
