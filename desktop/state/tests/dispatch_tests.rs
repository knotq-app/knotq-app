use chrono::NaiveDate;
use knotq_commands::Command;
use knotq_model::{AppSettings, NodeRef, Workspace};
use knotq_state::{AppEvent, AppState};

#[test]
fn apply_command_emits_event_marks_dirty_and_updates_index() {
    let mut state = test_state();
    let receiver = state.subscribe();
    let root = state.workspace.root;

    state.apply_command(Command::CreateFolder {
        parent: root,
        name: "Projects".into(),
        position: None,
    });

    assert!(state.is_dirty());
    assert_eq!(state.indexed.workspace.folders[&root].children.len(), 1);
    assert!(matches!(
        receiver.try_recv().unwrap(),
        AppEvent::WorkspaceChanged(_)
    ));
}

#[test]
fn selecting_scheme_updates_plain_selection() {
    let mut state = test_state();
    let scheme_id = knotq_model::SchemeId::new();

    state.select_node(NodeRef::Scheme(scheme_id));

    assert_eq!(state.selection.scheme_id, Some(scheme_id));
    assert_eq!(state.selection.view, knotq_state::View::Scheme);
}

fn test_state() -> AppState {
    AppState::new(
        Workspace::new(),
        AppSettings::default(),
        NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
        NaiveDate::from_ymd_opt(2025, 12, 1).unwrap(),
        false,
    )
}
