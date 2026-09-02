mod support;

use knotq_commands::{Command, WorkspaceCommandExt};
use knotq_mcp::Outcome;
use knotq_model::ItemId;
use serde_json::json;
use support::{is_unchanged, Fixture};

// ── The command each tool actually emits ──────────────────────────────────
//
// These pin the mapping itself. A tool that quietly starts emitting a
// different command still passes an end-state assertion if the end state
// happens to match; it does not pass this.

#[test]
fn add_item_emits_an_insert_carrying_the_id_it_reported() {
    let f = Fixture::new();
    let outcome = f
        .call(
            "add_item",
            json!({ "scheme_id": f.notes.to_string(), "text": "new line" }),
        )
        .unwrap();
    let Outcome::Write { command, response } = &outcome else {
        panic!("expected a write, got {outcome:?}");
    };
    let Command::InsertItem {
        scheme,
        position,
        item,
    } = command
    else {
        panic!("expected InsertItem, got {command:?}");
    };
    assert_eq!(*scheme, f.notes);
    assert_eq!(*position, 4, "no position means append");
    assert_eq!(item.text(), "new line");
    // The id in the response is the id in the command — that equality is what
    // makes a retry idempotent.
    assert_eq!(response["item"]["id"], item.id.to_string());
}

#[test]
fn update_item_emits_one_command_per_changed_field_and_none_for_the_rest() {
    let f = Fixture::new();
    let item = f.item_id(f.notes, 0);
    let outcome = f
        .call(
            "update_item",
            json!({
                "scheme_id": f.notes.to_string(),
                "item_id": item.to_string(),
                "text": "rewritten",
                "priority": 2,
                // Already a checkbox — must not produce a SetItemMarker.
                "marker": "checkbox",
                // Already this instant — must not produce a SetItemDate.
                "start": "2026-09-02T09:00:00Z",
            }),
        )
        .unwrap();
    let Some(Command::Batch(commands)) = outcome.command() else {
        panic!("expected a batch, got {:?}", outcome.command());
    };
    assert_eq!(commands.len(), 2, "got {commands:?}");
    assert!(matches!(commands[0], Command::UpdateItemText { .. }));
    assert!(matches!(
        commands[1],
        Command::SetItemPriority {
            priority: Some(2),
            ..
        }
    ));
}

#[test]
fn set_item_completed_emits_a_toggle_only_when_the_state_actually_differs() {
    let mut f = Fixture::new();
    let item = f.item_id(f.notes, 0);
    let args = json!({
        "scheme_id": f.notes.to_string(),
        "item_id": item.to_string(),
        "completed": true,
    });

    let first = f.call("set_item_completed", args.clone()).unwrap();
    assert!(matches!(
        first.command(),
        Some(Command::ToggleOccurrence { .. })
    ));
    f.run("set_item_completed", args.clone()).unwrap();

    // The underlying command is a toggle. Asking a second time for the state it
    // is already in must NOT emit one, or the retry un-completes the item.
    let second = f.call("set_item_completed", args).unwrap();
    assert!(is_unchanged(&second), "got {second:?}");
    assert!(f.scheme(f.notes).items[0].single_state().is_done());
}

#[test]
fn delete_scheme_archives_rather_than_destroying() {
    let f = Fixture::new();
    let outcome = f
        .call("delete_scheme", json!({ "scheme_id": f.notes.to_string() }))
        .unwrap();
    // Never `PermanentlyDeleteScheme`: an agent must not be able to make an
    // unrecoverable deletion on the user's behalf.
    assert!(matches!(
        outcome.command(),
        Some(Command::DeleteScheme { .. })
    ));
}

// ── Round trips: apply the command, read the result back ──────────────────

#[test]
fn add_item_then_read_scheme_shows_the_line_where_it_was_asked_for() {
    let mut f = Fixture::new();
    f.run(
        "add_item",
        json!({
            "scheme_id": f.notes.to_string(),
            "text": "inserted at the top",
            "position": 0,
            "marker": "bullet",
            "indent": 1,
            "priority": 3,
        }),
    )
    .unwrap();

    let out = f.read("read_scheme", json!({ "scheme_id": f.notes.to_string() }));
    let first = &out["items"][0];
    assert_eq!(first["text"], "inserted at the top");
    assert_eq!(first["marker"], "bullet");
    assert_eq!(first["indent"], 1);
    assert_eq!(first["priority"], 3);
    assert_eq!(out["items"].as_array().unwrap().len(), 5);
}

#[test]
fn a_dated_line_gets_a_checkbox_and_an_undated_one_gets_a_bullet() {
    let mut f = Fixture::new();
    f.run(
        "add_item",
        json!({ "scheme_id": f.notes.to_string(), "text": "dated", "start": "2026-09-04T10:00:00Z" }),
    )
    .unwrap();
    f.run(
        "add_item",
        json!({ "scheme_id": f.notes.to_string(), "text": "undated" }),
    )
    .unwrap();

    let out = f.read("read_scheme", json!({ "scheme_id": f.notes.to_string() }));
    let items = out["items"].as_array().unwrap();
    assert_eq!(items[4]["marker"], "checkbox");
    assert_eq!(items[5]["marker"], "bullet");
}

#[test]
fn update_item_clears_a_date_when_told_null_and_leaves_it_alone_when_omitted() {
    let mut f = Fixture::new();
    let item = f.item_id(f.notes, 0);

    // Omitted: untouched.
    f.run(
        "update_item",
        json!({
            "scheme_id": f.notes.to_string(),
            "item_id": item.to_string(),
            "text": "still dated",
        }),
    )
    .unwrap();
    assert!(f.scheme(f.notes).items[0].start.is_some());

    // Explicit null: cleared. This distinction is the whole reason
    // `Args::mentions` exists.
    f.run(
        "update_item",
        json!({
            "scheme_id": f.notes.to_string(),
            "item_id": item.to_string(),
            "start": serde_json::Value::Null,
        }),
    )
    .unwrap();
    assert!(f.scheme(f.notes).items[0].start.is_none());
}

#[test]
fn move_item_puts_the_line_at_the_requested_index() {
    let mut f = Fixture::new();
    let last = f.item_id(f.notes, 3);
    f.run(
        "move_item",
        json!({
            "scheme_id": f.notes.to_string(),
            "item_id": last.to_string(),
            "position": 0,
        }),
    )
    .unwrap();
    assert_eq!(f.scheme(f.notes).items[0].id, last);
}

#[test]
fn delete_item_removes_the_line() {
    let mut f = Fixture::new();
    let item = f.item_id(f.notes, 1);
    f.run(
        "delete_item",
        json!({ "scheme_id": f.notes.to_string(), "item_id": item.to_string() }),
    )
    .unwrap();
    assert_eq!(f.scheme(f.notes).items.len(), 3);
    assert!(f.scheme(f.notes).items.iter().all(|i| i.id != item));
}

#[test]
fn create_and_rename_a_scheme_round_trip_through_list_schemes() {
    let mut f = Fixture::new();
    f.run(
        "create_scheme",
        json!({ "name": "Fresh", "folder_id": f.folder.to_string() }),
    )
    .unwrap();

    let out = f.read("list_schemes", json!({}));
    let fresh = out["schemes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "Fresh")
        .expect("the new scheme should be listed");
    let id = fresh["id"].as_str().unwrap().to_string();

    f.run("rename_scheme", json!({ "scheme_id": id, "name": "Renamed" }))
        .unwrap();
    let out = f.read("list_schemes", json!({}));
    assert!(out["schemes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["name"] == "Renamed"));
}

#[test]
fn set_item_recurrence_sets_and_clears_a_repeat() {
    let mut f = Fixture::new();
    let item = f.item_id(f.notes, 0);
    f.run(
        "set_item_recurrence",
        json!({
            "scheme_id": f.notes.to_string(),
            "item_id": item.to_string(),
            "rrules": ["FREQ=WEEKLY;BYDAY=MO"],
        }),
    )
    .unwrap();
    assert_eq!(
        f.scheme(f.notes).items[0]
            .repeats
            .as_ref()
            .unwrap()
            .rrules,
        vec!["FREQ=WEEKLY;BYDAY=MO"]
    );

    f.run(
        "set_item_recurrence",
        json!({
            "scheme_id": f.notes.to_string(),
            "item_id": item.to_string(),
            "rrules": serde_json::Value::Null,
        }),
    )
    .unwrap();
    assert!(f.scheme(f.notes).items[0].repeats.is_none());
}

/// Exception dates record instances the user deleted. Re-setting the rule must
/// not resurrect them.
#[test]
fn changing_a_repeat_rule_keeps_the_exceptions_already_on_the_line() {
    let mut f = Fixture::new();
    let item = f.item_id(f.notes, 2);
    let exdate = knotq_model::CalendarDateTime::utc(support::at(2026, 9, 3, 9, 30));
    f.workspace
        .schemes
        .get_mut(&f.notes)
        .unwrap()
        .items[2]
        .repeats
        .as_mut()
        .unwrap()
        .exdates
        .push(exdate.clone());

    f.run(
        "set_item_recurrence",
        json!({
            "scheme_id": f.notes.to_string(),
            "item_id": item.to_string(),
            "rrules": ["FREQ=WEEKLY"],
        }),
    )
    .unwrap();

    let repeats = f.scheme(f.notes).items[2].repeats.as_ref().unwrap();
    assert_eq!(repeats.rrules, vec!["FREQ=WEEKLY"]);
    assert_eq!(repeats.exdates, vec![exdate]);
}

// ── Idempotency ───────────────────────────────────────────────────────────

#[test]
fn replaying_add_item_with_the_same_id_does_not_add_a_second_copy() {
    let mut f = Fixture::new();
    let id = ItemId::new();
    let args = json!({
        "scheme_id": f.notes.to_string(),
        "text": "exactly once",
        "item_id": id.to_string(),
    });

    let first = f.run("add_item", args.clone()).unwrap();
    assert_eq!(first["created"], json!(true));
    assert_eq!(f.scheme(f.notes).items.len(), 5);

    // The client never saw the first response and retried. This is the ordinary
    // case for an agent over a flaky connection, not an exotic one.
    let second = f.run("add_item", args).unwrap();
    assert_eq!(second["created"], json!(false));
    assert_eq!(f.scheme(f.notes).items.len(), 5);
    assert_eq!(second["item"]["id"], id.to_string());
}

#[test]
fn replaying_a_delete_reports_success_rather_than_a_missing_line() {
    let mut f = Fixture::new();
    let item = f.item_id(f.notes, 1);
    let args = json!({ "scheme_id": f.notes.to_string(), "item_id": item.to_string() });
    f.run("delete_item", args.clone()).unwrap();

    let again = f.call("delete_item", args).unwrap();
    assert!(is_unchanged(&again));
}

#[test]
fn a_no_op_update_reports_the_line_rather_than_failing() {
    let f = Fixture::new();
    let item = f.item_id(f.notes, 3);
    let outcome = f
        .call(
            "update_item",
            json!({
                "scheme_id": f.notes.to_string(),
                "item_id": item.to_string(),
                "text": "a plain note",
            }),
        )
        .unwrap();
    assert!(is_unchanged(&outcome));
}

// ── Refusals ──────────────────────────────────────────────────────────────

#[test]
fn every_write_tool_refuses_a_linked_calendar() {
    let f = Fixture::new();
    let item = f.item_id(f.calendar, 0);
    let scheme = f.calendar.to_string();
    let cases: Vec<(&str, serde_json::Value)> = vec![
        ("rename_scheme", json!({ "scheme_id": scheme, "name": "x" })),
        ("add_item", json!({ "scheme_id": scheme, "text": "x" })),
        (
            "update_item",
            json!({ "scheme_id": scheme, "item_id": item.to_string(), "text": "x" }),
        ),
        (
            "delete_item",
            json!({ "scheme_id": scheme, "item_id": item.to_string() }),
        ),
        (
            "move_item",
            json!({ "scheme_id": scheme, "item_id": item.to_string(), "position": 0 }),
        ),
        (
            "set_item_recurrence",
            json!({ "scheme_id": scheme, "item_id": item.to_string(), "rrules": ["FREQ=DAILY"] }),
        ),
    ];
    for (name, args) in cases {
        let err = f
            .call(name, args)
            .unwrap_err_or_else(|| panic!("`{name}` should refuse a linked calendar"));
        assert_eq!(err.kind(), "refused", "`{name}` gave the wrong kind");
        assert!(
            err.to_string().contains("read-only"),
            "`{name}` must say why: {err}"
        );
    }
}

#[test]
fn writes_are_refused_in_read_only_mode_before_a_command_is_built() {
    let f = Fixture::new();
    let err = f
        .call_as(
            "add_item",
            json!({ "scheme_id": f.notes.to_string(), "text": "nope" }),
            true,
        )
        .unwrap_err();
    assert_eq!(err.kind(), "refused");
    assert!(err.to_string().contains("read-only mode"));
}

#[test]
fn an_archived_scheme_must_be_restored_by_the_user_before_an_agent_edits_it() {
    let f = Fixture::new();
    let err = f
        .call(
            "add_item",
            json!({ "scheme_id": f.archived.to_string(), "text": "x" }),
        )
        .unwrap_err();
    assert_eq!(err.kind(), "refused");
    assert!(err.to_string().contains("archive"));
}

#[test]
fn completing_a_repeating_line_requires_naming_which_instance() {
    let f = Fixture::new();
    let standup = f.item_id(f.notes, 2);
    let err = f
        .call(
            "set_item_completed",
            json!({
                "scheme_id": f.notes.to_string(),
                "item_id": standup.to_string(),
                "completed": true,
            }),
        )
        .unwrap_err();
    assert_eq!(err.kind(), "invalid_params");
    assert!(err.to_string().contains("occurrence"));
}

#[test]
fn completing_one_instance_of_a_repeat_leaves_the_others_open() {
    let mut f = Fixture::new();
    let out = f.read("list_upcoming", json!({ "limit": 10 }));
    let standups: Vec<&serde_json::Value> = out["occurrences"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|o| o["text"] == "standup")
        .collect();
    let target = standups[0]["occurrence"].as_str().unwrap().to_string();
    let item_id = standups[0]["item_id"].as_str().unwrap().to_string();

    f.run(
        "set_item_completed",
        json!({
            "scheme_id": f.notes.to_string(),
            "item_id": item_id,
            "occurrence": target,
            "completed": true,
        }),
    )
    .unwrap();

    let after = f.read("list_upcoming", json!({ "limit": 10 }));
    let done: Vec<bool> = after["occurrences"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|o| o["text"] == "standup")
        .map(|o| o["done"].as_bool().unwrap())
        .collect();
    assert_eq!(done.iter().filter(|d| **d).count(), 1, "got {done:?}");
    assert!(done.len() > 1, "the other instances should still be listed");
}

#[test]
fn an_occurrence_handle_the_agent_invented_is_refused() {
    let f = Fixture::new();
    let standup = f.item_id(f.notes, 2);
    let err = f
        .call(
            "set_item_completed",
            json!({
                "scheme_id": f.notes.to_string(),
                "item_id": standup.to_string(),
                "occurrence": "2026-09-02T09:30:00Z",
                "completed": true,
            }),
        )
        .unwrap_err();
    assert_eq!(err.kind(), "invalid_params");
}

#[test]
fn a_line_cannot_be_made_to_repeat_without_a_start_date() {
    let f = Fixture::new();
    let undated = f.item_id(f.notes, 3);
    let err = f
        .call(
            "set_item_recurrence",
            json!({
                "scheme_id": f.notes.to_string(),
                "item_id": undated.to_string(),
                "rrules": ["FREQ=DAILY"],
            }),
        )
        .unwrap_err();
    assert_eq!(err.kind(), "refused");
}

#[test]
fn out_of_range_values_are_named_in_the_error() {
    let f = Fixture::new();
    for (field, args) in [
        (
            "priority",
            json!({ "scheme_id": f.notes.to_string(), "text": "x", "priority": 99 }),
        ),
        (
            "indent",
            json!({ "scheme_id": f.notes.to_string(), "text": "x", "indent": 40 }),
        ),
    ] {
        let err = f.call("add_item", args).unwrap_err();
        assert_eq!(err.kind(), "invalid_params");
        assert!(err.to_string().contains(field), "got {err}");
    }
}

#[test]
fn a_position_past_the_end_is_refused_rather_than_silently_appended() {
    let f = Fixture::new();
    let err = f
        .call(
            "add_item",
            json!({ "scheme_id": f.notes.to_string(), "text": "x", "position": 99 }),
        )
        .unwrap_err();
    assert_eq!(err.kind(), "invalid_params");
}

#[test]
fn the_workspace_root_folder_cannot_be_deleted() {
    let f = Fixture::new();
    let err = f
        .call(
            "delete_folder",
            json!({ "folder_id": f.workspace.root.to_string() }),
        )
        .unwrap_err();
    assert_eq!(err.kind(), "refused");
}

#[test]
fn deleting_a_folder_reports_how_many_schemes_it_takes_with_it() {
    let mut f = Fixture::new();
    let out = f
        .run("delete_folder", json!({ "folder_id": f.folder.to_string() }))
        .unwrap();
    assert_eq!(out["schemes_archived"], json!(3));
}

/// A malformed id is the client's mistake and must come back as such, so the
/// model corrects its next call instead of concluding the item vanished.
#[test]
fn a_malformed_id_is_an_invalid_param_not_a_missing_item() {
    let f = Fixture::new();
    let err = f
        .call("read_scheme", json!({ "scheme_id": "the-notes-one" }))
        .unwrap_err();
    assert_eq!(err.kind(), "invalid_params");
}

trait UnwrapErrOrElse<T, E> {
    fn unwrap_err_or_else(self, f: impl FnOnce() -> E) -> E;
}

impl<T: std::fmt::Debug, E> UnwrapErrOrElse<T, E> for Result<T, E> {
    fn unwrap_err_or_else(self, f: impl FnOnce() -> E) -> E {
        match self {
            Ok(_) => f(),
            Err(e) => e,
        }
    }
}

// ── Reporting what was created ────────────────────────────────────────────

/// `CreateScheme` mints its own id, so the only place it exists after an apply
/// is the receipt's inverse. Without this the agent has to re-list and guess,
/// and two schemes can share a name.
#[test]
fn the_id_of_a_created_scheme_is_recoverable_from_the_receipt() {
    let mut f = Fixture::new();
    let outcome = f
        .call("create_scheme", json!({ "name": "Fresh" }))
        .unwrap();
    let Outcome::Write { command, .. } = outcome else {
        panic!("expected a write");
    };
    let receipt = f
        .workspace
        .apply_with_origin(command, knotq_commands::CommandOrigin::Agent)
        .unwrap();

    let (field, id) = knotq_mcp::created_id(&receipt.inverse).expect("an id to report");
    assert_eq!(field, "scheme_id");
    assert!(f.workspace.schemes.contains_key(&id.parse().unwrap()));
}

#[test]
fn a_created_folders_id_is_recoverable_too() {
    let mut f = Fixture::new();
    let Outcome::Write { command, .. } = f
        .call("create_folder", json!({ "name": "Projects" }))
        .unwrap()
    else {
        panic!("expected a write");
    };
    let receipt = f
        .workspace
        .apply_with_origin(command, knotq_commands::CommandOrigin::Agent)
        .unwrap();

    let (field, id) = knotq_mcp::created_id(&receipt.inverse).expect("an id to report");
    assert_eq!(field, "folder_id");
    assert!(f.workspace.folders.contains_key(&id.parse().unwrap()));
}

/// An edit is not a creation. Reporting a `scheme_id` for a rename would have
/// the agent treat an existing scheme as newly made.
#[test]
fn a_command_that_created_nothing_reports_no_id() {
    let mut f = Fixture::new();
    let Outcome::Write { command, .. } = f
        .call(
            "rename_scheme",
            json!({ "scheme_id": f.notes.to_string(), "name": "Renamed" }),
        )
        .unwrap()
    else {
        panic!("expected a write");
    };
    let receipt = f
        .workspace
        .apply_with_origin(command, knotq_commands::CommandOrigin::Agent)
        .unwrap();
    assert!(knotq_mcp::created_id(&receipt.inverse).is_none());
}
