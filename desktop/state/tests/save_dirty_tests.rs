//! Every scheme a command creates or rewrites must reach the save.
//!
//! The save task writes only `dirty_schemes` when any are set (an incremental
//! save), while the workspace index is always rewritten. A scheme that is in the
//! index but was never marked dirty therefore has no file on disk — and one
//! missing scheme file makes the whole workspace fail to load on the next launch.
//! The production-path sync fuzzer found this through a crash right after an
//! ordinary save.

use std::collections::HashMap;

use chrono::NaiveDate;
use knotq_commands::{Command, CommandOrigin};
use knotq_model::{AppSettings, Item, NodeRef, Scheme, Workspace};
use knotq_state::AppState;
use knotq_storage_json::{load_workspace, save_workspace, save_workspace_incremental};

fn temp_workspace_path(name: &str) -> std::path::PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir()
        .join(format!(
            "knotq-save-dirty-{name}-{}-{unique}",
            std::process::id()
        ))
        .join("workspace")
        .join("workspace.json")
}

fn day() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 9, 14).unwrap()
}

/// A saved workspace with one scheme holding one line, and the state over it.
fn saved_state(path: &std::path::Path) -> (AppState, knotq_model::SchemeId, knotq_model::ItemId) {
    let mut workspace = Workspace::new();
    let mut scheme = Scheme::new("Existing", 0);
    let item = Item::new("line");
    let item_id = item.id;
    scheme.items.push(item);
    let scheme_id = scheme.id;
    workspace
        .folders
        .get_mut(&workspace.root)
        .unwrap()
        .children
        .push(NodeRef::Scheme(scheme_id));
    workspace.schemes.insert(scheme_id, scheme);
    workspace.ensure_sync_metadata();
    save_workspace(path, &workspace).unwrap();
    let state = AppState::new(
        workspace,
        AppSettings::default(),
        day(),
        day(),
        false,
        HashMap::<knotq_model::DocumentId, Vec<u8>>::new(),
        1,
    );
    (state, scheme_id, item_id)
}

/// What the save task does once edits are pending (`write_save_snapshot`).
fn save_like_the_save_task(state: &mut AppState, path: &std::path::Path) {
    let dirty = std::mem::take(&mut state.dirty_schemes);
    state.index_dirty = false;
    if dirty.is_empty() {
        save_workspace(path, &state.workspace).unwrap();
    } else {
        save_workspace_incremental(path, &state.workspace, &dirty).unwrap();
    }
}

#[test]
fn a_created_scheme_is_saved_alongside_another_schemes_edit() {
    let path = temp_workspace_path("create");
    let (mut state, existing, item) = saved_state(&path);
    let root = state.workspace.root;

    state
        .apply_prechecked_local_command(
            Command::CreateScheme {
                folder: root,
                name: "Brand new".into(),
                color_index: 2,
                position: None,
            },
            CommandOrigin::User,
        )
        .unwrap();
    state
        .apply_prechecked_local_command(
            Command::UpdateItemText {
                scheme: existing,
                item,
                text: "edited in the same save window".into(),
            },
            CommandOrigin::User,
        )
        .unwrap();
    save_like_the_save_task(&mut state, &path);

    let loaded = load_workspace(&path)
        .expect("a completed save must leave a loadable workspace")
        .expect("workspace present");
    assert!(loaded
        .schemes
        .values()
        .any(|scheme| scheme.name == "Brand new"));
    let _ = std::fs::remove_dir_all(path.parent().unwrap().parent().unwrap());
}

#[test]
fn a_restored_scheme_is_saved_alongside_another_schemes_edit() {
    let path = temp_workspace_path("restore");
    let (mut state, existing, item) = saved_state(&path);
    let root = state.workspace.root;
    let mut restored = Scheme::new("Restored", 3);
    restored.items.push(Item::new("came back"));

    state
        .apply_prechecked_local_command(
            Command::RestoreScheme {
                folder: root,
                position: 0,
                scheme: restored,
            },
            CommandOrigin::Importer,
        )
        .unwrap();
    state
        .apply_prechecked_local_command(
            Command::UpdateItemText {
                scheme: existing,
                item,
                text: "edited in the same save window".into(),
            },
            CommandOrigin::User,
        )
        .unwrap();
    save_like_the_save_task(&mut state, &path);

    let loaded = load_workspace(&path)
        .expect("a completed save must leave a loadable workspace")
        .expect("workspace present");
    let scheme = loaded
        .schemes
        .values()
        .find(|scheme| scheme.name == "Restored")
        .expect("restored scheme present");
    assert_eq!(scheme.items[0].text(), "came back");
    let _ = std::fs::remove_dir_all(path.parent().unwrap().parent().unwrap());
}
