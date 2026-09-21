//! Daily Queue page creation, and the global-uniqueness rule it has to respect.

use chrono::NaiveDate;
use knotq_commands::{Command, WorkspaceCommandExt};
use knotq_model::{daily_queue_placeholder_item_id, NodeRef, Scheme, Workspace};

mod support;

fn date() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 9, 16).expect("valid date")
}

#[test]
fn an_empty_day_gets_its_blank_placeholder_row() {
    let mut workspace = Workspace::new();
    workspace
        .apply(Command::EnsureDailyQueue { date: date() })
        .expect("create the day");

    let scheme = workspace.daily_queue_scheme_id(date()).expect("bound day");
    let items = &workspace.schemes[&scheme].items;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, daily_queue_placeholder_item_id(date()));
    assert!(items[0].content.as_text().is_some_and(str::is_empty));
}

#[test]
fn the_day_is_idempotent_and_keeps_its_content() {
    let mut workspace = Workspace::new();
    workspace
        .apply(Command::EnsureDailyQueue { date: date() })
        .expect("create the day");
    let scheme = workspace.daily_queue_scheme_id(date()).expect("bound day");
    workspace
        .apply(Command::UpdateItemText {
            scheme,
            item: daily_queue_placeholder_item_id(date()),
            text: "buy milk".into(),
        })
        .expect("type on the day");

    workspace
        .apply(Command::EnsureDailyQueue { date: date() })
        .expect("re-open the day");

    assert_eq!(workspace.schemes[&scheme].items.len(), 1);
    assert_eq!(workspace.schemes[&scheme].items[0].text(), "buy milk");
}

/// An item id is globally unique. The placeholder's id is derived from the
/// date so two devices creating the same day converge on one blank row — but
/// that also makes it re-mintable after the row has *moved* to another page
/// (a carry-over, or an ordinary drag). Re-creating it would put one id in two
/// schemes at once, which the CRDT resolves by hiding the loser: the row
/// becomes invisible to every document-derived view while the plain workspace
/// still shows it, so the day silently empties again on the next sync.
#[test]
fn a_day_whose_placeholder_moved_away_is_not_given_a_second_copy_of_it() {
    let mut workspace = Workspace::new();
    let root = workspace.root;
    let elsewhere = Scheme::new("Inbox", 0);
    let elsewhere_id = elsewhere.id;
    workspace.schemes.insert(elsewhere_id, elsewhere);
    workspace
        .folders
        .get_mut(&root)
        .expect("root")
        .children
        .push(NodeRef::Scheme(elsewhere_id));

    workspace
        .apply(Command::EnsureDailyQueue { date: date() })
        .expect("create the day");
    let day = workspace.daily_queue_scheme_id(date()).expect("bound day");
    let placeholder = daily_queue_placeholder_item_id(date());
    let row = workspace.schemes[&day].items[0].clone();

    // Move the row off the day, exactly as a carry-over or a drag does: a
    // delete in the source document and the same id inserted in the target.
    workspace
        .apply(Command::Batch(vec![
            Command::DeleteItem {
                scheme: day,
                item: placeholder,
            },
            Command::InsertItem {
                scheme: elsewhere_id,
                position: 0,
                item: row,
            },
        ]))
        .expect("move the row");
    assert!(workspace.schemes[&day].items.is_empty());

    workspace
        .apply(Command::EnsureDailyQueue { date: date() })
        .expect("re-open the now-empty day");

    assert!(
        workspace.schemes[&day].items.is_empty(),
        "the day must stay blank rather than mint a second copy of a row that \
         is alive in another scheme"
    );
    assert_eq!(
        workspace.schemes[&elsewhere_id]
            .items
            .iter()
            .filter(|item| item.id == placeholder)
            .count(),
        1,
        "the moved row stays where the user put it, exactly once"
    );
}
