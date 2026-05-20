use chrono::NaiveDate;
use knotq_commands::Command;
use knotq_model::{AppSettings, Workspace};
use knotq_state::{should_coalesce_editor_undo, AppState, EditorUndoGroup, EditorUndoKey};
use std::time::{Duration, Instant};

#[test]
fn undo_and_redo_restore_workspace_shape() {
    let mut state = test_state();
    let root = state.workspace.root;

    state.apply_command(Command::CreateFolder {
        parent: root,
        name: "Projects".into(),
        position: None,
    });
    assert_eq!(state.workspace.folders[&root].children.len(), 1);

    state.undo_command();
    assert!(state.workspace.folders[&root].children.is_empty());

    state.redo_command();
    assert_eq!(state.workspace.folders[&root].children.len(), 1);
}

#[test]
fn editor_undo_coalesces_inside_time_window() {
    let key = EditorUndoKey {
        scheme_id: knotq_model::SchemeId::new(),
        item_id: knotq_model::ItemId::new(),
    };
    let now = Instant::now();
    let group = EditorUndoGroup {
        key,
        last_edit: now - Duration::from_millis(100),
    };

    assert!(should_coalesce_editor_undo(Some(key), Some(group), now));
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
