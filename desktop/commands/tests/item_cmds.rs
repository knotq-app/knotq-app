use chrono::Utc;
use knotq_commands::{Command, DateKind, WorkspaceCommandExt};
use knotq_model::{Item, ItemKind, ItemMarker, OccurrenceId, Workspace};

mod support;

use support::create_root_scheme;

#[test]
fn create_and_toggle_item_with_undo() {
    let mut workspace = Workspace::new();
    let scheme_id = create_root_scheme(&mut workspace);
    let item = Item::new("hello");
    let item_id = item.id;
    workspace
        .apply(Command::InsertItem {
            scheme: scheme_id,
            position: 0,
            item,
        })
        .unwrap();

    let toggle = workspace
        .apply(Command::ToggleOccurrence {
            scheme: scheme_id,
            item: item_id,
            occurrence: OccurrenceId::Single,
        })
        .unwrap();

    assert!(workspace.schemes[&scheme_id].items[0].state[0]
        .state
        .is_done());

    workspace.apply(toggle.inverse).unwrap();

    assert!(!workspace.schemes[&scheme_id].items[0].state[0]
        .state
        .is_done());
}

#[test]
fn toggle_occurrence_promotes_non_checkbox_and_undo_restores_marker() {
    let mut workspace = Workspace::new();
    let scheme_id = create_root_scheme(&mut workspace);
    let mut item = Item::new("hello");
    item.marker = ItemMarker::Bullet;
    let item_id = item.id;
    workspace
        .apply(Command::InsertItem {
            scheme: scheme_id,
            position: 0,
            item,
        })
        .unwrap();

    let toggle = workspace
        .apply(Command::ToggleOccurrence {
            scheme: scheme_id,
            item: item_id,
            occurrence: OccurrenceId::Single,
        })
        .unwrap();

    let item = &workspace.schemes[&scheme_id].items[0];
    assert_eq!(item.marker, ItemMarker::Checkbox);
    assert!(item.state[0].state.is_done());

    workspace.apply(toggle.inverse).unwrap();

    let item = &workspace.schemes[&scheme_id].items[0];
    assert_eq!(item.marker, ItemMarker::Bullet);
    assert_eq!(item.state[0].state.progress, 0);
}

#[test]
fn set_occurrence_notification_offset_is_undoable() {
    let mut workspace = Workspace::new();
    let scheme_id = create_root_scheme(&mut workspace);
    let item = Item::new("hello").with_start(Utc::now());
    let item_id = item.id;
    workspace
        .apply(Command::InsertItem {
            scheme: scheme_id,
            position: 0,
            item,
        })
        .unwrap();

    let receipt = workspace
        .apply(Command::SetOccurrenceNotificationOffset {
            scheme: scheme_id,
            item: item_id,
            occurrence: OccurrenceId::Single,
            offset_secs: Some(-600),
        })
        .unwrap();

    assert_eq!(
        workspace.schemes[&scheme_id].items[0]
            .single_state()
            .notification_offset_secs,
        Some(-600)
    );

    workspace.apply(receipt.inverse).unwrap();

    assert_eq!(
        workspace.schemes[&scheme_id].items[0]
            .single_state()
            .notification_offset_secs,
        None
    );
}

#[test]
fn replace_item_is_undoable() {
    let mut workspace = Workspace::new();
    let scheme_id = create_root_scheme(&mut workspace);
    let dated = Item::new("").with_indent(2).with_start(Utc::now()).done();
    let item_id = dated.id;
    workspace
        .apply(Command::InsertItem {
            scheme: scheme_id,
            position: 0,
            item: dated,
        })
        .unwrap();

    let mut clean = Item::new("");
    clean.id = item_id;
    let replace = workspace
        .apply(Command::ReplaceItem {
            scheme: scheme_id,
            item: clean,
        })
        .unwrap();

    let item = &workspace.schemes[&scheme_id].items[0];
    assert_eq!(item.indent, 0);
    assert!(item.start.is_none());
    assert!(!item.state[0].state.is_done());

    workspace.apply(replace.inverse).unwrap();

    let item = &workspace.schemes[&scheme_id].items[0];
    assert_eq!(item.indent, 2);
    assert!(item.start.is_some());
    assert!(item.state[0].state.is_done());
}

#[test]
fn marker_constraints_clear_dates_for_non_checkbox_items() {
    let mut workspace = Workspace::new();
    let scheme_id = create_root_scheme(&mut workspace);
    let mut item = Item::new("plain").with_start(Utc::now());
    item.marker = ItemMarker::Blank;
    let item_id = item.id;
    workspace
        .apply(Command::InsertItem {
            scheme: scheme_id,
            position: 0,
            item,
        })
        .unwrap();

    let item = &workspace.schemes[&scheme_id].items[0];
    assert_eq!(item.id, item_id);
    assert_eq!(item.marker, ItemMarker::Blank);
    assert!(item.start.is_none());
    assert_eq!(item.kind(), ItemKind::Procedure);
}

#[test]
fn setting_date_promotes_non_checkbox_to_checkbox() {
    let mut workspace = Workspace::new();
    let scheme_id = create_root_scheme(&mut workspace);
    let mut item = Item::new("plain");
    item.marker = ItemMarker::Bullet;
    let item_id = item.id;
    workspace
        .apply(Command::InsertItem {
            scheme: scheme_id,
            position: 0,
            item,
        })
        .unwrap();

    let date = Utc::now();
    let receipt = workspace
        .apply(Command::SetItemDate {
            scheme: scheme_id,
            item: item_id,
            kind: DateKind::Start,
            date: Some(date),
        })
        .unwrap();

    let item = &workspace.schemes[&scheme_id].items[0];
    assert_eq!(item.marker, ItemMarker::Checkbox);
    assert_eq!(item.start, Some(date));

    workspace.apply(receipt.inverse).unwrap();

    let item = &workspace.schemes[&scheme_id].items[0];
    assert_eq!(item.marker, ItemMarker::Bullet);
    assert!(item.start.is_none());
}

#[test]
fn removing_checkbox_marker_clears_date_annotations_and_undo_restores() {
    let mut workspace = Workspace::new();
    let scheme_id = create_root_scheme(&mut workspace);
    let item = Item::new("dated").with_start(Utc::now()).done();
    let item_id = item.id;
    workspace
        .apply(Command::InsertItem {
            scheme: scheme_id,
            position: 0,
            item,
        })
        .unwrap();

    let receipt = workspace
        .apply(Command::SetItemMarker {
            scheme: scheme_id,
            item: item_id,
            marker: ItemMarker::Bullet,
        })
        .unwrap();

    let item = &workspace.schemes[&scheme_id].items[0];
    assert_eq!(item.marker, ItemMarker::Bullet);
    assert!(item.start.is_none());
    assert_eq!(item.kind(), ItemKind::Procedure);

    workspace.apply(receipt.inverse).unwrap();

    let item = &workspace.schemes[&scheme_id].items[0];
    assert_eq!(item.marker, ItemMarker::Checkbox);
    assert!(item.start.is_some());
}

/// An item's marker family has to be one the line's marker can actually draw.
///
/// Not a cosmetic rule: the plain scheme file stores the marker and its family
/// as one token and drops a family the marker cannot use, while the CRDT
/// document stores `marker_family` as a field of its own and keeps whatever it
/// is given. A line carrying a family its marker rejects therefore cannot
/// round-trip through disk, and the two halves of the data directory disagree
/// about it for good — which the next sync reads as a local edit and re-asserts
/// one over the other, with no other device involved.
#[test]
fn changing_a_marker_drops_a_family_the_new_marker_cannot_draw() {
    use knotq_model::MarkerFamily;

    let mut workspace = Workspace::new();
    let scheme_id = create_root_scheme(&mut workspace);
    let item = Item::new("a line");
    let item_id = item.id;
    workspace
        .apply(Command::InsertItem {
            scheme: scheme_id,
            position: 0,
            item,
        })
        .unwrap();
    workspace
        .apply(Command::SetItemMarker {
            scheme: scheme_id,
            item: item_id,
            marker: ItemMarker::Bullet,
        })
        .unwrap();
    workspace
        .apply(Command::SetItemMarkerFamily {
            scheme: scheme_id,
            item: item_id,
            family: MarkerFamily::Rings,
        })
        .unwrap();
    let line = |workspace: &Workspace| workspace.schemes[&scheme_id].items[0].clone();
    assert_eq!(line(&workspace).marker_family, MarkerFamily::Rings);

    // A ring is a bullet glyph; a checkbox cannot draw one.
    workspace
        .apply(Command::SetItemMarker {
            scheme: scheme_id,
            item: item_id,
            marker: ItemMarker::Checkbox,
        })
        .unwrap();

    let line = line(&workspace);
    assert_eq!(line.marker, ItemMarker::Checkbox);
    assert_eq!(
        line.marker_family,
        MarkerFamily::Standard,
        "a family the marker cannot draw must not survive the marker change: it \
         is unrepresentable in the scheme file and would diverge from the CRDT"
    );
    assert_eq!(
        line.marker_token(),
        "checkbox",
        "the written token must round-trip to the value the model holds"
    );
}

/// The same rule stated over the whole workspace, for content that arrives from
/// somewhere other than a command (a pull, an import, an older build).
#[test]
fn normalizing_markers_reports_the_schemes_it_repaired() {
    use knotq_model::MarkerFamily;

    let mut workspace = Workspace::new();
    let scheme_id = create_root_scheme(&mut workspace);
    let mut item = Item::new("a line");
    item.marker = ItemMarker::Checkbox;
    item.marker_family = MarkerFamily::Rings;
    workspace
        .schemes
        .get_mut(&scheme_id)
        .unwrap()
        .items
        .push(item);

    let repaired = workspace.normalize_item_markers();

    assert!(
        repaired.contains(&scheme_id),
        "the caller has to know which schemes to write, not merely that something changed"
    );
    assert_eq!(
        workspace.schemes[&scheme_id].items[0].marker_family,
        MarkerFamily::Standard
    );
    assert!(
        workspace.normalize_item_markers().is_empty(),
        "normalization is idempotent"
    );
}

/// Completing a recurring occurrence and then un-completing it must leave the
/// item exactly as it was. A default entry for a recurring occurrence says
/// nothing that its absence does not, and the sync path prunes it from the copy
/// it writes into the CRDT documents — so keeping one here would leave the
/// plain workspace holding a value its own documents never did (production fuzz
/// seed 10005).
#[test]
fn un_completing_a_recurring_occurrence_leaves_no_husk_behind() {
    use chrono::TimeZone;

    let mut workspace = Workspace::new();
    let scheme_id = create_root_scheme(&mut workspace);
    let mut item = Item::new("a repeating line");
    item.marker = ItemMarker::Checkbox;
    let item_id = item.id;
    workspace
        .apply(Command::InsertItem {
            scheme: scheme_id,
            position: 0,
            item,
        })
        .unwrap();

    let before = workspace.schemes[&scheme_id].items[0].state.clone();
    let occurrence = OccurrenceId::Recurring {
        original_start: knotq_model::CalendarDateTime::utc(
            Utc.with_ymd_and_hms(2026, 9, 15, 7, 0, 0).unwrap(),
        ),
    };

    for _ in 0..2 {
        workspace
            .apply(Command::ToggleOccurrence {
                scheme: scheme_id,
                item: item_id,
                occurrence: occurrence.clone(),
            })
            .unwrap();
    }

    assert_eq!(
        workspace.schemes[&scheme_id].items[0].state, before,
        "a round trip through done and back must not be observable"
    );
}
