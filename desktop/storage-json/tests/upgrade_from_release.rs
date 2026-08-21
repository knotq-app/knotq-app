//! Opening a real released install with this build.
//!
//! Each fixture under `tests/fixtures/` is a data directory captured by running
//! that release's own code, so these tests answer the only question that
//! matters before shipping a format change: if someone updates from that
//! version, does everything they have still come back?
//!
//! "Everything" is deliberately broad — their writing, but also the parts they
//! never see and would not notice going missing until it costs them: the CRDT
//! document identity their other devices recognise, the edits queued for the
//! next push, the account the app is signed into.

mod support;

use std::collections::HashMap;
use std::fs;

use knotq_model::{DocumentId, Workspace};
use knotq_storage_json::{
    crdt_state_dir, crdt_state_path, load_app_settings, load_crdt_state, load_local_sync_state,
    load_workspace, run_pending_upgrades, save_crdt_state, save_workspace, DATA_LAYOUT_VERSION,
};
use support::*;

/// Everything a user would notice missing, in a form two loads can be compared by.
#[derive(Debug, PartialEq)]
struct Contents {
    /// Scheme name → its lines, rendered as they are stored.
    schemes: Vec<(String, Vec<String>)>,
    daily_queue: Vec<String>,
    archived: Vec<String>,
    folders: Vec<String>,
}

fn contents_of(workspace: &Workspace) -> Contents {
    let mut schemes: Vec<(String, Vec<String>)> = workspace
        .schemes
        .values()
        .map(|scheme| {
            (
                scheme.name.clone(),
                scheme.items.iter().map(describe_item).collect(),
            )
        })
        .collect();
    schemes.sort();
    let mut daily_queue: Vec<String> = workspace
        .daily_queue
        .keys()
        .map(|date| date.to_string())
        .collect();
    daily_queue.sort();
    let mut archived: Vec<String> = workspace
        .iter_deleted_schemes()
        .map(|scheme| scheme.name.clone())
        .collect();
    archived.sort();
    let mut folders: Vec<String> = workspace
        .folders
        .values()
        .map(|folder| folder.name.clone())
        .collect();
    folders.sort();
    Contents {
        schemes,
        daily_queue,
        archived,
        folders,
    }
}

/// Text alone would let an image or a table silently become an empty line.
fn describe_item(item: &knotq_model::Item) -> String {
    let body = match &item.content {
        knotq_model::ItemContent::Text { text } => format!("text:{text}"),
        knotq_model::ItemContent::Image(image) => {
            format!("image:{}:{:?}", image.asset, image.format)
        }
        knotq_model::ItemContent::Table(table) => {
            format!("table:{}x{}", table.columns.len(), table.rows.len())
        }
    };
    format!(
        "{body}|marker={:?}|indent={}|start={:?}|end={:?}|repeats={}|state={:?}",
        item.marker,
        item.indent,
        item.start,
        item.end,
        item.repeats.is_some(),
        item.state
    )
}

fn load_contents(data_dir: &std::path::Path) -> Contents {
    let workspace = load_workspace(&workspace_path_in(data_dir))
        .expect("the workspace must load")
        .expect("the fixture has a workspace");
    contents_of(&workspace)
}

/// The user's writing. If this ever fails, someone shipped a format change that
/// eats notes.
#[test]
fn an_upgraded_install_keeps_every_scheme_and_line() {
    for (release, fixture) in release_fixtures() {
        let data_dir = open_fixture(&fixture, "contents");
        let before = load_contents(&data_dir);

        let report = run_pending_upgrades(&workspace_path_in(&data_dir));
        assert!(
            report.is_clean(),
            "{release}: upgrade was not clean: {report:?}"
        );

        assert_eq!(
            load_contents(&data_dir),
            before,
            "{release}: the workspace changed across the upgrade"
        );

        // Not just "some schemes": the specific things this fixture holds, so a
        // loader that silently returned an empty workspace cannot pass.
        let names: Vec<&str> = before
            .schemes
            .iter()
            .map(|(name, _)| name.as_str())
            .collect();
        for expected in ["Notes", "Health", "Media", "Team calendar"] {
            assert!(
                names.contains(&expected),
                "{release}: scheme {expected} is missing from {names:?}"
            );
        }
        assert!(
            before.archived.contains(&"Old project".to_string()),
            "{release}: the archived scheme is missing"
        );
        assert!(
            before.folders.contains(&"Work".to_string()),
            "{release}: the nested folder is missing"
        );
        assert_eq!(
            before.daily_queue.len(),
            2,
            "{release}: both daily-queue days must survive"
        );

        let lines: Vec<&String> = before
            .schemes
            .iter()
            .flat_map(|(_, lines)| lines.iter())
            .collect();
        for expected in [
            "A plain line",
            "Unicode: 日本語 — emoji 🧶",
            "Markdown **bold**",
            "table:2x1",
        ] {
            assert!(
                lines.iter().any(|line| line.contains(expected)),
                "{release}: no line containing {expected:?}"
            );
        }

        cleanup(&data_dir);
    }
}

/// The part a user cannot see and cannot rebuild. A document that comes back
/// under a new identity does not merge with what the server holds — it re-seeds
/// the account, and the other devices' history stops matching.
#[test]
fn an_upgraded_install_keeps_its_sync_identity() {
    for (release, fixture) in release_fixtures() {
        let data_dir = open_fixture(&fixture, "identity");
        let workspace_path = workspace_path_in(&data_dir);

        let before: HashMap<DocumentId, Vec<u8>> = load_crdt_state(&workspace_path).unwrap();
        assert!(
            !before.is_empty(),
            "{release}: the fixture must carry CRDT state or this test proves nothing"
        );
        let sync_before = load_local_sync_state(&workspace_path).unwrap();
        assert_eq!(
            sync_before.pending.len(),
            1,
            "{release}: the fixture must carry an unpushed edit"
        );

        run_pending_upgrades(&workspace_path);

        let after = load_crdt_state(&workspace_path).unwrap();
        assert_eq!(
            after, before,
            "{release}: the persisted CRDT documents changed across the upgrade"
        );

        let sync_after = load_local_sync_state(&workspace_path).unwrap();
        assert_eq!(
            sync_after.pending, sync_before.pending,
            "{release}: an unpushed edit was lost"
        );
        assert_eq!(sync_after.document_cursors, sync_before.document_cursors);
        assert_eq!(sync_after.media_cursors, sync_before.media_cursors);
        assert_eq!(sync_after.workspace_id, sync_before.workspace_id);
        assert_eq!(
            sync_after.replica_id, sync_before.replica_id,
            "{release}: the replica identity must not be re-minted"
        );

        cleanup(&data_dir);
    }
}

#[test]
fn an_upgraded_install_keeps_its_account_and_preferences() {
    for (release, fixture) in release_fixtures() {
        let data_dir = open_fixture(&fixture, "settings");
        let settings_path = data_dir.join("settings.json");
        let before = load_app_settings(&settings_path).unwrap();

        run_pending_upgrades(&workspace_path_in(&data_dir));

        let after = load_app_settings(&settings_path).unwrap();
        assert_eq!(
            serde_json::to_value(&after).unwrap(),
            serde_json::to_value(&before).unwrap(),
            "{release}: settings changed across the upgrade"
        );

        let account = after
            .sync_account
            .as_ref()
            .unwrap_or_else(|| panic!("{release}: the signed-in account was lost"));
        assert_eq!(account.email, "user@example.com");
        assert!(
            !account.bearer_token.is_empty(),
            "{release}: the session token was lost, which signs the user out"
        );
        assert_eq!(
            after.google_accounts.len(),
            1,
            "{release}: the linked Google account was lost"
        );
        assert_eq!(after.theme_mode, knotq_model::ThemeMode::Dark);

        cleanup(&data_dir);
    }
}

/// Loading is only half of it: the first save is what replaces the user's files.
#[test]
fn saving_after_the_upgrade_does_not_drop_anything() {
    for (release, fixture) in release_fixtures() {
        let data_dir = open_fixture(&fixture, "resave");
        let workspace_path = workspace_path_in(&data_dir);
        run_pending_upgrades(&workspace_path);

        let workspace = load_workspace(&workspace_path).unwrap().unwrap();
        let crdt = load_crdt_state(&workspace_path).unwrap();
        let index_keys_before = json_key_paths(&read_json(&workspace_path));

        save_workspace(&workspace_path, &workspace).unwrap();
        save_crdt_state(&workspace_path, &crdt).unwrap();

        let reloaded = load_workspace(&workspace_path).unwrap().unwrap();
        assert_eq!(
            reloaded, workspace,
            "{release}: the workspace did not survive a save/load round trip"
        );
        assert_eq!(
            load_crdt_state(&workspace_path).unwrap(),
            crdt,
            "{release}: CRDT state did not survive a save/load round trip"
        );

        // A key that the fixture's index had and ours no longer writes is a
        // field this build would drop for every user who upgrades.
        let index_keys_after = json_key_paths(&read_json(&workspace_path));
        let dropped: Vec<&String> = index_keys_before
            .iter()
            .filter(|key| !index_keys_after.contains(*key))
            .collect();
        assert!(
            dropped.is_empty(),
            "{release}: saving dropped index fields that the release wrote: {dropped:?}"
        );

        cleanup(&data_dir);
    }
}

/// The old form is evidence. Keeping it is what makes a bad upgrade recoverable
/// by hand instead of by restore-from-backup.
#[test]
fn the_upgrade_keeps_the_old_form_and_records_what_it_did() {
    for (release, fixture) in release_fixtures() {
        let data_dir = open_fixture(&fixture, "record");
        let workspace_path = workspace_path_in(&data_dir);
        let legacy_blob = crdt_state_path(&workspace_path);
        assert!(
            legacy_blob.exists(),
            "{release}: the fixture must start in the pre-migration form"
        );
        let original = fs::read(&legacy_blob).unwrap();

        let report = run_pending_upgrades(&workspace_path);

        assert_eq!(
            report.applied,
            vec!["crdt-state-per-document-files"],
            "{release}: unexpected migration set"
        );
        assert!(
            crdt_state_dir(&workspace_path).is_dir(),
            "{release}: the new form was not written"
        );
        assert!(
            !legacy_blob.exists(),
            "{release}: the old blob must not keep shadowing the directory"
        );
        let retired = data_dir.join("sync-crdt-state.json.migrated");
        assert_eq!(
            fs::read(&retired).unwrap(),
            original,
            "{release}: the pre-migration state must be kept, byte for byte"
        );

        let record: serde_json::Value = read_json(&data_dir.join("data-layout.json"));
        assert_eq!(record["layout_version"], DATA_LAYOUT_VERSION);
        assert_eq!(record["applied"][0], "crdt-state-per-document-files");
        assert!(record["last_written_by"].as_str().is_some());

        // And a copy of what it touched, before it touched it.
        let backups: Vec<_> = fs::read_dir(data_dir.join("upgrade-backups"))
            .unwrap()
            .flatten()
            .collect();
        assert_eq!(backups.len(), 1, "{release}: expected one backup");
        assert_eq!(
            fs::read(backups[0].path().join("sync-crdt-state.json")).unwrap(),
            original
        );

        cleanup(&data_dir);
    }
}

/// Launching twice must not do the migration twice, and must not undo it.
#[test]
fn the_upgrade_is_idempotent_across_relaunches() {
    for (release, fixture) in release_fixtures() {
        let data_dir = open_fixture(&fixture, "idempotent");
        let workspace_path = workspace_path_in(&data_dir);

        let first = run_pending_upgrades(&workspace_path);
        assert_eq!(first.applied.len(), 1);
        let after_first = load_crdt_state(&workspace_path).unwrap();
        let state_dir_after_first = snapshot_tree(&crdt_state_dir(&workspace_path));

        for launch in 2..=4 {
            let report = run_pending_upgrades(&workspace_path);
            assert!(
                report.applied.is_empty() && report.resumed.is_empty() && report.is_clean(),
                "{release}: launch {launch} re-ran a migration: {report:?}"
            );
            assert_eq!(
                snapshot_tree(&crdt_state_dir(&workspace_path)),
                state_dir_after_first,
                "{release}: launch {launch} rewrote the CRDT state directory"
            );
        }
        assert_eq!(load_crdt_state(&workspace_path).unwrap(), after_first);

        cleanup(&data_dir);
    }
}

fn read_json(path: &std::path::Path) -> serde_json::Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

/// Every `a.b.c` path in a JSON document, with array indices collapsed and map
/// keys that are ids replaced by `*`, so a different number of schemes does not
/// read as a different shape.
fn json_key_paths(value: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    walk(value, String::new(), &mut out);
    out.sort();
    out.dedup();
    return out;

    fn walk(value: &serde_json::Value, prefix: String, out: &mut Vec<String>) {
        match value {
            serde_json::Value::Object(map) => {
                for (key, child) in map {
                    let key = if is_uuid_like(key) || key.parse::<i64>().is_ok() {
                        "*"
                    } else {
                        key.as_str()
                    };
                    let path = if prefix.is_empty() {
                        key.to_string()
                    } else {
                        format!("{prefix}.{key}")
                    };
                    out.push(path.clone());
                    walk(child, path, out);
                }
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    walk(item, format!("{prefix}[]"), out);
                }
            }
            _ => {}
        }
    }

    fn is_uuid_like(key: &str) -> bool {
        key.len() == 36 && key.chars().filter(|c| *c == '-').count() == 4
    }
}
