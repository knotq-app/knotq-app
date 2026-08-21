//! The rules every migration is run under, tested once so that every future
//! migration inherits the coverage instead of re-deriving it.
//!
//! These are the failure modes that do not happen on a developer's machine and
//! do happen on users': a laptop that sleeps and never wakes mid-upgrade, a file
//! that has gone bad, an install that has already seen a newer build.

mod support;

use std::fs;

use knotq_storage_json::{
    crdt_state_dir, crdt_state_path, load_crdt_state, registered_migrations, run_pending_upgrades,
    DATA_LAYOUT_VERSION,
};
use support::*;

const JOURNAL: &str = "data-upgrade-journal.json";
const LAYOUT: &str = "data-layout.json";

fn a_fixture(label: &str) -> std::path::PathBuf {
    let (_, fixture) = release_fixtures().into_iter().next().unwrap();
    open_fixture(&fixture, label)
}

/// The registry's own invariants. Ids are written into users' `data-layout.json`,
/// so a rename or a reorder makes every install believe it has work to do.
#[test]
fn the_registry_stays_append_only_and_versioned() {
    let all = registered_migrations();
    let ids: Vec<&str> = all.iter().map(|migration| migration.id).collect();
    let mut unique = ids.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), ids.len(), "duplicate migration id in {ids:?}");

    for migration in all {
        assert!(!migration.id.is_empty());
        assert!(
            migration
                .id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c == '-'),
            "migration id {:?} should be kebab-case",
            migration.id
        );
        assert!(
            !migration.summary.is_empty(),
            "migration {} needs a summary — it is what a support log will quote",
            migration.id
        );
    }

    assert_eq!(
        all.len() as u32,
        DATA_LAYOUT_VERSION,
        "adding a migration must bump DATA_LAYOUT_VERSION, so a directory it has \
         run against is recognisable as newer by builds that predate it"
    );
}

/// The first launch on a machine that has never run KnotQ.
#[test]
fn a_fresh_install_has_nothing_to_upgrade() {
    let dir = std::env::temp_dir().join(format!(
        "knotq-upgrade-fresh-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let report = run_pending_upgrades(&workspace_path_in(&dir));
    assert!(report.is_clean());
    assert!(report.applied.is_empty());
    assert!(
        !dir.exists(),
        "an upgrade run must not create the data directory before the app does"
    );

    // And once the directory exists but holds nothing to migrate, the run is
    // still a no-op — but it stamps the version, so a later downgrade can tell.
    fs::create_dir_all(&dir).unwrap();
    let report = run_pending_upgrades(&workspace_path_in(&dir));
    assert!(report.is_clean() && report.applied.is_empty());
    let record: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(dir.join(LAYOUT)).unwrap()).unwrap();
    assert_eq!(record["layout_version"], DATA_LAYOUT_VERSION);

    cleanup(&dir);
}

/// Killed between writing the new form and retiring the old one. The journal is
/// what makes this recoverable: without it the next launch sees a directory that
/// looks migrated and a blob that looks stale.
#[test]
fn an_upgrade_killed_part_way_through_finishes_on_the_next_launch() {
    let data_dir = a_fixture("resumed");
    let workspace_path = workspace_path_in(&data_dir);
    let expected = load_crdt_state(&workspace_path).unwrap();
    assert!(expected.len() > 1);

    // Reproduce the disk state of a process that died mid-migration: the journal
    // names the migration, one document made it out, the blob is still there.
    fs::write(
        data_dir.join(JOURNAL),
        serde_json::to_string(&["crdt-state-per-document-files"]).unwrap(),
    )
    .unwrap();
    let dir = crdt_state_dir(&workspace_path);
    fs::create_dir_all(&dir).unwrap();
    let (first, bytes) = expected.iter().next().unwrap();
    fs::write(dir.join(format!("{first}.ydoc")), bytes).unwrap();

    let report = run_pending_upgrades(&workspace_path);

    assert_eq!(
        report.resumed,
        vec!["crdt-state-per-document-files"],
        "the unfinished migration must be recognised as resumed: {report:?}"
    );
    assert!(report.is_clean(), "{report:?}");
    assert_eq!(
        load_crdt_state(&workspace_path).unwrap(),
        expected,
        "every document must be present after the resumed migration"
    );
    assert!(
        !data_dir.join(JOURNAL).exists(),
        "the journal must be cleared once the migration completes"
    );

    cleanup(&data_dir);
}

/// A migration that cannot do its job must leave no trace. Half-migrated is the
/// state nothing else in the system knows how to reason about.
#[test]
fn a_migration_that_cannot_finish_leaves_everything_as_it_was() {
    let data_dir = a_fixture("rollback");
    let workspace_path = workspace_path_in(&data_dir);

    // A blob this build cannot parse: `apply` fails on the very first step.
    fs::write(crdt_state_path(&workspace_path), b"{ not json").unwrap();
    let before = snapshot_tree(&data_dir);

    let report = run_pending_upgrades(&workspace_path);

    assert_eq!(report.failed.len(), 1, "{report:?}");
    assert_eq!(report.failed[0].0, "crdt-state-per-document-files");
    assert!(!report.is_clean());
    assert!(
        report.applied.is_empty(),
        "a failed migration must not be reported as applied"
    );

    assert_eq!(
        fs::read(crdt_state_path(&workspace_path)).unwrap(),
        b"{ not json",
        "the original file must be exactly as it was"
    );
    assert!(
        !crdt_state_dir(&workspace_path).exists(),
        "a rolled-back migration must not leave the new form behind"
    );

    // Everything the migration did not declare must be untouched, and the files
    // it did declare must be back to their original bytes.
    let after = snapshot_tree(&data_dir);
    for (path, bytes) in &before {
        let found = after.iter().find(|(other, _)| other == path);
        assert_eq!(
            found.map(|(_, bytes)| bytes),
            Some(bytes),
            "{} changed across a rolled-back migration",
            path.display()
        );
    }

    cleanup(&data_dir);
}

/// Rolling back to an older build, or opening a synced folder from a machine on
/// a newer one. This build cannot represent what is there, so it must not write.
#[test]
fn a_data_directory_from_a_newer_build_is_left_untouched() {
    let data_dir = a_fixture("newer");
    let workspace_path = workspace_path_in(&data_dir);
    fs::write(
        data_dir.join(LAYOUT),
        serde_json::json!({
            "layout_version": DATA_LAYOUT_VERSION + 7,
            "applied": ["crdt-state-per-document-files", "something-this-build-never-heard-of"],
            "last_written_by": "9.9.9"
        })
        .to_string(),
    )
    .unwrap();
    let before = snapshot_tree(&data_dir);

    let report = run_pending_upgrades(&workspace_path);

    assert_eq!(report.written_by_newer_build, Some(DATA_LAYOUT_VERSION + 7));
    assert!(
        !report.is_clean(),
        "the caller must be told to keep the session read-only"
    );
    assert!(report.applied.is_empty());
    assert_eq!(
        snapshot_tree(&data_dir),
        before,
        "not one byte may change in a directory from a newer build"
    );

    cleanup(&data_dir);
}

/// The record is a cache of what has been done, never the authority. Losing it
/// must not re-run a migration whose old form is already gone.
#[test]
fn an_unreadable_layout_record_does_not_redo_finished_work() {
    let data_dir = a_fixture("lost-record");
    let workspace_path = workspace_path_in(&data_dir);
    run_pending_upgrades(&workspace_path);
    let migrated = load_crdt_state(&workspace_path).unwrap();
    let state_dir = snapshot_tree(&crdt_state_dir(&workspace_path));

    fs::write(data_dir.join(LAYOUT), b"corrupt").unwrap();
    let report = run_pending_upgrades(&workspace_path);

    assert!(report.is_clean(), "{report:?}");
    assert!(report.applied.is_empty(), "nothing was left to migrate");
    assert_eq!(load_crdt_state(&workspace_path).unwrap(), migrated);
    assert_eq!(snapshot_tree(&crdt_state_dir(&workspace_path)), state_dir);

    // And the record is rewritten, so the next launch is cheap again.
    let record: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(data_dir.join(LAYOUT)).unwrap()).unwrap();
    assert_eq!(record["layout_version"], DATA_LAYOUT_VERSION);
    assert_eq!(record["applied"][0], "crdt-state-per-document-files");

    cleanup(&data_dir);
}

/// A directory restored from a backup (or copied off an old machine) is in the
/// old form again while the record says it was migrated. The old form present on
/// disk always wins over the record.
#[test]
fn a_directory_rolled_back_to_the_old_form_is_migrated_again() {
    let data_dir = a_fixture("rolled-back");
    let workspace_path = workspace_path_in(&data_dir);
    let original_blob = fs::read(crdt_state_path(&workspace_path)).unwrap();
    let expected = load_crdt_state(&workspace_path).unwrap();

    run_pending_upgrades(&workspace_path);

    // Put the old form back, as restoring a backup over the directory would.
    fs::remove_dir_all(crdt_state_dir(&workspace_path)).unwrap();
    fs::write(crdt_state_path(&workspace_path), &original_blob).unwrap();

    let report = run_pending_upgrades(&workspace_path);

    assert_eq!(report.applied, vec!["crdt-state-per-document-files"]);
    assert_eq!(load_crdt_state(&workspace_path).unwrap(), expected);

    cleanup(&data_dir);
}
