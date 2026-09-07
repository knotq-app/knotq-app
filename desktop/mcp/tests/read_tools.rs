mod support;

use serde_json::json;
use support::{at, Fixture};

#[test]
fn list_schemes_hides_the_archive_by_default_and_flags_read_only_calendars() {
    let f = Fixture::new();
    let out = f.read("list_schemes", json!({}));
    let names: Vec<&str> = out["schemes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["Notes", "Team calendar"]);

    let calendar = out["schemes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "Team calendar")
        .unwrap();
    // The agent is told up front rather than discovering it by being refused.
    assert_eq!(calendar["read_only"], json!(true));
    assert_eq!(calendar["folder_path"], json!(["Work"]));

    let with_archive = f.read("list_schemes", json!({ "include_archived": true }));
    assert_eq!(with_archive["schemes"].as_array().unwrap().len(), 3);
}

/// A `HashMap` gives a different iteration order every run. An agent that lists
/// twice and sees a reshuffle will assume the workspace changed under it.
#[test]
fn list_schemes_is_ordered_the_same_way_every_time() {
    let f = Fixture::new();
    let first = f.read("list_schemes", json!({}));
    for _ in 0..20 {
        assert_eq!(f.read("list_schemes", json!({})), first);
    }
}

#[test]
fn read_scheme_returns_lines_in_document_order_with_their_fields() {
    let f = Fixture::new();
    let out = f.read("read_scheme", json!({ "scheme_id": f.notes.to_string() }));
    let items = out["items"].as_array().unwrap();
    assert_eq!(items.len(), 4);
    assert_eq!(items[0]["text"], "write the proposal");
    assert_eq!(items[0]["marker"], "checkbox");
    assert_eq!(items[0]["start"], "2026-09-02T09:00:00Z");
    assert_eq!(items[2]["recurrence"], json!(["FREQ=DAILY"]));
    assert_eq!(items[3]["marker"], "blank");
    // A line with no date carries no date keys at all, rather than nulls the
    // model has to interpret.
    assert!(items[3].get("start").is_none());
}

#[test]
fn read_scheme_can_drop_completed_lines() {
    let mut f = Fixture::new();
    let item = f.item_id(f.notes, 0);
    f.run(
        "set_item_completed",
        json!({ "scheme_id": f.notes.to_string(), "item_id": item.to_string(), "completed": true }),
    )
    .unwrap();

    let all = f.read("read_scheme", json!({ "scheme_id": f.notes.to_string() }));
    assert_eq!(all["items"].as_array().unwrap().len(), 4);

    let open = f.read(
        "read_scheme",
        json!({ "scheme_id": f.notes.to_string(), "include_completed": false }),
    );
    let texts: Vec<&str> = open["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["text"].as_str().unwrap())
        .collect();
    assert!(!texts.contains(&"write the proposal"));
    assert_eq!(texts.len(), 3);
}

#[test]
fn read_scheme_on_a_stale_id_says_so_instead_of_failing_the_transport() {
    let f = Fixture::new();
    let err = f
        .call(
            "read_scheme",
            json!({ "scheme_id": knotq_model::SchemeId::new().to_string() }),
        )
        .unwrap_err();
    assert_eq!(err.kind(), "not_found");
    // Not a protocol error: the model should see this and re-list, not have the
    // client surface it as a broken connection.
    assert!(!err.is_protocol_error());
}

#[test]
fn search_finds_lines_and_returns_the_ids_needed_to_edit_them() {
    let f = Fixture::new();
    let out = f.read("search", json!({ "query": "proposal" }));
    let results = out["results"].as_array().unwrap();
    assert!(!results.is_empty());
    let hit = &results[0];
    assert_eq!(hit["scheme_id"], f.notes.to_string());
    assert!(hit["item_id"].is_string());
}

#[test]
fn list_upcoming_starts_at_the_given_instant_and_expands_repeats() {
    let f = Fixture::new();
    let out = f.read("list_upcoming", json!({ "limit": 10 }));
    let occurrences = out["occurrences"].as_array().unwrap();
    assert!(!occurrences.is_empty());
    // Nothing before `from` — the overdue line at 2026-08-20 must not appear.
    for o in occurrences {
        assert!(o["start"].as_str().unwrap() >= "2026-09-01T12:00:00Z");
    }
    // The daily standup repeats, so it shows more than once in a 10-item window.
    let standups = occurrences.iter().filter(|o| o["text"] == "standup").count();
    assert!(standups > 1, "expected the daily repeat to expand, got {standups}");
}

#[test]
fn a_repeating_occurrence_carries_a_handle_that_differs_per_instance() {
    let f = Fixture::new();
    let out = f.read("list_upcoming", json!({ "limit": 10 }));
    let handles: Vec<&str> = out["occurrences"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|o| o["text"] == "standup")
        .map(|o| o["occurrence"].as_str().unwrap())
        .collect();
    assert!(handles.len() > 1);
    assert_ne!(handles[0], handles[1]);
    // And none of them is the base handle, which would complete every instance.
    assert!(handles.iter().all(|h| *h != "single"));
}

#[test]
fn list_overdue_reports_the_past_due_line() {
    let f = Fixture::new();
    let out = f.read("list_overdue", json!({}));
    let texts: Vec<&str> = out["occurrences"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["text"].as_str().unwrap())
        .collect();
    assert!(texts.contains(&"overdue thing"), "got {texts:?}");
}

#[test]
fn list_calendar_covers_an_explicit_range_and_admits_when_it_truncates() {
    let f = Fixture::new();
    let out = f.read(
        "list_calendar",
        json!({ "start": "2026-09-01T00:00:00Z", "end": "2026-09-03T00:00:00Z" }),
    );
    assert_eq!(out["truncated"], json!(false));
    let occurrences = out["occurrences"].as_array().unwrap();
    assert!(!occurrences.is_empty());

    let capped = f.read(
        "list_calendar",
        json!({
            "start": "2026-09-01T00:00:00Z",
            "end": "2026-12-01T00:00:00Z",
            "limit": 2,
        }),
    );
    assert_eq!(capped["occurrences"].as_array().unwrap().len(), 2);
    // Without this an agent summarises two days as if they were three months.
    assert_eq!(capped["truncated"], json!(true));
    assert!(capped["total_matching"].as_u64().unwrap() > 2);
}

#[test]
fn list_calendar_rejects_a_backwards_range_rather_than_returning_nothing() {
    let f = Fixture::new();
    let err = f
        .call(
            "list_calendar",
            json!({ "start": "2026-09-03T00:00:00Z", "end": "2026-09-01T00:00:00Z" }),
        )
        .unwrap_err();
    assert_eq!(err.kind(), "invalid_params");
}

#[test]
fn a_timestamp_without_an_offset_is_refused_rather_than_guessed_at() {
    let f = Fixture::new();
    let err = f
        .call("list_upcoming", json!({ "from": "2026-09-01T09:00:00" }))
        .unwrap_err();
    assert_eq!(err.kind(), "invalid_params");
    assert!(
        err.to_string().contains("offset"),
        "the message must say what is missing: {err}"
    );
}

#[test]
fn get_daily_queue_reports_an_empty_day_rather_than_an_error() {
    let f = Fixture::new();
    let out = f.read("get_daily_queue", json!({ "date": "2026-09-05" }));
    assert!(out["scheme"].is_null());
    assert_eq!(out["items"], json!([]));
}

#[test]
fn get_daily_queue_returns_the_days_scheme_when_there_is_one() {
    let mut f = Fixture::new();
    let day = chrono::NaiveDate::from_ymd_opt(2026, 9, 1).unwrap();
    f.workspace.daily_queue.insert(day, f.notes);

    let out = f.read("get_daily_queue", json!({}));
    assert_eq!(out["scheme"]["id"], f.notes.to_string());
    assert_eq!(out["items"].as_array().unwrap().len(), 4);
}

/// The daily-queue index can name a scheme that is no longer present. "What's
/// on today" must not read as a server fault when that happens.
#[test]
fn get_daily_queue_survives_an_index_entry_whose_scheme_is_gone() {
    let mut f = Fixture::new();
    let day = chrono::NaiveDate::from_ymd_opt(2026, 9, 1).unwrap();
    f.workspace
        .daily_queue
        .insert(day, knotq_model::SchemeId::new());

    let out = f.read("get_daily_queue", json!({}));
    assert!(out["scheme"].is_null());
}

#[test]
fn an_over_large_limit_is_capped_rather_than_refused() {
    let f = Fixture::new();
    let out = f.read("list_upcoming", json!({ "limit": 100_000 }));
    assert!(out["occurrences"].as_array().unwrap().len() <= 200);
}

#[test]
fn reads_are_allowed_in_read_only_mode() {
    let f = Fixture::new();
    assert!(f
        .call_as("list_schemes", json!({}), true)
        .is_ok());
}

#[test]
fn an_unknown_tool_is_a_protocol_error_so_the_client_learns_it_called_wrong() {
    let f = Fixture::new();
    let err = f.call("summon_a_pony", json!({})).unwrap_err();
    assert_eq!(err.kind(), "unknown_tool");
    assert!(err.is_protocol_error());
}

#[test]
fn the_fixture_clock_is_the_one_the_tools_use() {
    let f = Fixture::new();
    let out = f.read("list_upcoming", json!({}));
    assert_eq!(out["from"], "2026-09-01T12:00:00Z");
    let _ = at(2026, 9, 1, 12, 0);
}

/// A workspace accumulates one scheme per planned day. In a listing they are
/// indistinguishable from real documents unless they say so, and an agent asked
/// to "add this to my notes" would have hundreds of equally plausible targets.
#[test]
fn a_days_plan_is_labelled_as_one_and_a_real_document_is_not() {
    let mut f = Fixture::new();
    let day = chrono::NaiveDate::from_ymd_opt(2026, 8, 30).unwrap();
    let mut daily = knotq_model::Scheme::new("Daily 2026-08-30", 0);
    let daily_id = daily.id;
    daily.items.push(knotq_model::Item::new("plan for the day"));
    f.workspace.schemes.insert(daily_id, daily);
    f.workspace
        .folders
        .get_mut(&f.folder)
        .unwrap()
        .children
        .push(knotq_model::NodeRef::Scheme(daily_id));
    f.workspace.daily_queue.insert(day, daily_id);

    let out = f.read("list_schemes", json!({}));
    let schemes = out["schemes"].as_array().unwrap();
    let listed_daily = schemes
        .iter()
        .find(|s| s["id"] == daily_id.to_string())
        .expect("the day's plan should be listed");
    assert_eq!(listed_daily["daily_queue_date"], "2026-08-30");

    // A standing document carries no date key at all.
    let notes = schemes
        .iter()
        .find(|s| s["id"] == f.notes.to_string())
        .unwrap();
    assert!(notes.get("daily_queue_date").is_none());
}
