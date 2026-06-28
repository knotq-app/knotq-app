use knotq_commands::{Command, CommandError, CommandOrigin, WorkspaceCommandExt};
use knotq_model::{
    CalendarProvider, ImportedCalendarSource, Item, NodeRef, OccurrenceId, Scheme, SchemeId,
    SchemeSource, Workspace,
};

#[test]
fn user_item_mutations_are_rejected_for_read_only_calendar_schemes() {
    let mut workspace = Workspace::new();
    let scheme_id = insert_imported_scheme(&mut workspace, vec![Item::new("meeting")]);
    let item_id = workspace.schemes[&scheme_id].items[0].id;

    let err = workspace
        .apply(Command::UpdateItemText {
            scheme: scheme_id,
            item: item_id,
            text: "renamed".into(),
        })
        .unwrap_err();

    assert_read_only(err, scheme_id);
    assert_eq!(workspace.schemes[&scheme_id].items[0].text(), "meeting");

    let err = workspace
        .apply(Command::DeleteItem {
            scheme: scheme_id,
            item: item_id,
        })
        .unwrap_err();

    assert_read_only(err, scheme_id);
    assert_eq!(workspace.schemes[&scheme_id].items.len(), 1);
}

/// Completion is local-only state that never syncs back to the calendar, so an
/// occurrence can be toggled done even on a read-only imported scheme — unlike
/// the content edits rejected above.
#[test]
fn user_can_toggle_completion_on_read_only_calendar_schemes() {
    let mut workspace = Workspace::new();
    let scheme_id = insert_imported_scheme(&mut workspace, vec![Item::new("meeting")]);
    let item_id = workspace.schemes[&scheme_id].items[0].id;

    workspace
        .apply(Command::ToggleOccurrence {
            scheme: scheme_id,
            item: item_id,
            occurrence: OccurrenceId::Single,
        })
        .unwrap();
    assert!(workspace.schemes[&scheme_id].items[0]
        .single_state()
        .is_done());

    // Toggling again clears it back to not-done.
    workspace
        .apply(Command::ToggleOccurrence {
            scheme: scheme_id,
            item: item_id,
            occurrence: OccurrenceId::Single,
        })
        .unwrap();
    assert!(!workspace.schemes[&scheme_id].items[0]
        .single_state()
        .is_done());
}

#[test]
fn user_can_rename_and_recolor_read_only_calendar_schemes_but_not_change_source() {
    let mut workspace = Workspace::new();
    let scheme_id = insert_imported_scheme(&mut workspace, vec![]);

    workspace
        .apply(Command::RenameScheme {
            id: scheme_id,
            name: "New name".into(),
        })
        .unwrap();

    assert_eq!(workspace.schemes[&scheme_id].name, "New name");

    workspace
        .apply(Command::SetSchemeColor {
            id: scheme_id,
            color_index: 5,
        })
        .unwrap();

    assert_eq!(workspace.schemes[&scheme_id].color_index, 5);

    let err = workspace
        .apply(Command::SetSchemeSource {
            id: scheme_id,
            source: SchemeSource::Local,
        })
        .unwrap_err();

    assert_read_only(err, scheme_id);
    assert!(workspace.schemes[&scheme_id].is_read_only());
}

#[test]
fn importer_can_refresh_read_only_calendar_schemes() {
    let mut workspace = Workspace::new();
    let scheme_id = insert_imported_scheme(&mut workspace, vec![Item::new("meeting")]);
    let item_id = workspace.schemes[&scheme_id].items[0].id;

    workspace
        .apply_with_origin(
            Command::UpdateItemText {
                scheme: scheme_id,
                item: item_id,
                text: "updated from google".into(),
            },
            CommandOrigin::Importer,
        )
        .unwrap();

    assert_eq!(
        workspace.schemes[&scheme_id].items[0].text(),
        "updated from google"
    );
}

fn insert_imported_scheme(workspace: &mut Workspace, items: Vec<Item>) -> SchemeId {
    let mut scheme = Scheme::new("Imported", 3);
    scheme.source = SchemeSource::ImportedCalendar(ImportedCalendarSource {
        provider: CalendarProvider::Google,
        account_id: "account".into(),
        account_email: None,
        calendar_id: "calendar".into(),
        sync_token: None,
        read_only: true,
        last_synced_at: None,
    });
    scheme.items = items;
    let scheme_id = scheme.id;
    workspace.schemes.insert(scheme_id, scheme);
    workspace
        .folders
        .get_mut(&workspace.root)
        .unwrap()
        .children
        .push(NodeRef::Scheme(scheme_id));
    scheme_id
}

fn assert_read_only(err: CommandError, scheme_id: SchemeId) {
    assert!(matches!(err, CommandError::ReadOnlyScheme(id) if id == scheme_id));
}
