//! Larger sync stress scenarios for the batched CRDT engine.
//!
//! These tests intentionally cross client batching limits and mix workspace-shape
//! changes with scheme content changes. They still run entirely in-process through
//! the real sync engine and CRDT materialization layer.

mod common;

use chrono::NaiveDate;
use common::{DeviceKey, Harness, Rng, D0, D1};
use knotq_model::SchemeId;

#[test]
fn pushes_more_than_one_document_batch_without_losing_new_schemes() {
    let mut h = Harness::new(2);
    h.login_all();

    let before_pushes = h.server_push_calls();
    let mut schemes = Vec::new();
    for i in 0..90 {
        schemes.push(h.add_scheme(D0, &format!("bulk-{i:03}"), &["seed"]));
    }

    h.sync(D0);
    let push_batches = h.server_push_calls() - before_pushes;
    assert!(
        push_batches >= 2,
        "expected the 64-document push cap to require multiple requests, got {push_batches}",
    );
    assert_eq!(
        h.server_document_count(),
        schemes.len() + 1,
        "server should hold one workspace document plus every scheme document",
    );

    h.sync(D1);
    h.settle();
    h.assert_all_converged();
    for scheme in schemes {
        assert!(
            h.device(D1).workspace.schemes.contains_key(&scheme),
            "late peer lost bulk-created scheme {scheme}",
        );
    }
}

#[test]
fn pushes_more_than_one_update_batch_for_a_single_hot_document() {
    let mut h = Harness::new(2);
    h.login_all();
    let scheme = h.add_scheme(D0, "hot document", &["seed"]);
    h.settle();

    let before_pushes = h.server_push_calls();
    for i in 0..125 {
        h.append_line(D0, scheme, &format!("offline edit {i:03}"));
    }

    h.sync(D0);
    let push_batches = h.server_push_calls() - before_pushes;
    assert_eq!(
        push_batches, 3,
        "125 queued updates for one document should push as 50 + 50 + 25",
    );

    h.sync(D1);
    h.settle();
    h.assert_all_converged();
    let texts = h.device(D1).scheme_item_texts(scheme);
    assert_eq!(texts.len(), 126);
    assert_eq!(texts.first().map(String::as_str), Some("seed"));
    assert_eq!(texts.last().map(String::as_str), Some("offline edit 124"));
}

#[test]
fn fresh_device_pulls_large_mixed_workspace_with_archives_and_imported_calendars() {
    let mut h = Harness::new(2);
    h.login_all();

    let mut root_schemes = Vec::new();
    for i in 0..72 {
        root_schemes.push(h.add_scheme(D0, &format!("root-{i:03}"), &["seed"]));
    }

    let mut archived_folders = Vec::new();
    for folder_idx in 0..8 {
        let folder = h.add_folder(D0, &format!("archive-folder-{folder_idx}"));
        let mut child_schemes = Vec::new();
        for scheme_idx in 0..4 {
            child_schemes.push(h.add_scheme_to_folder(
                D0,
                folder,
                &format!("archived-{folder_idx}-{scheme_idx}"),
                &["folder item"],
            ));
        }
        h.archive_folder(D0, folder);
        archived_folders.push((folder, child_schemes));
    }

    let mut calendars = Vec::new();
    for i in 0..24 {
        calendars.push(h.import_calendar_scheme(
            D0,
            &format!("Calendar {i:02}"),
            &format!("google-account-{i:02}"),
            &format!("calendar-{i:02}@example.com"),
            &format!("calendar-id-{i:02}"),
            &["standup", "focus block", "review"],
        ));
    }

    let mut daily_entries = Vec::new();
    for day in 1..=18 {
        let date = NaiveDate::from_ymd_opt(2026, 7, day).unwrap();
        let daily = h.set_daily_queue(D0, date, &["carry", "plan"]);
        daily_entries.push((date, daily));
    }

    h.sync(D0);
    assert!(
        h.server_document_count()
            >= 1 + root_schemes.len() + 8 * 4 + calendars.len() + daily_entries.len(),
        "server did not retain the expected large document set",
    );

    // D1 starts from only the account skeleton and must discover every document
    // through merged-state pull, including archived folder subtrees and read-only
    // calendar metadata.
    h.sync(D1);
    h.settle();
    h.assert_all_converged();

    for (folder, schemes) in archived_folders {
        h.assert_archived_folder_with_schemes(D1, folder, &schemes);
    }
    for (i, calendar) in calendars.into_iter().enumerate() {
        let source = h
            .imported_calendar_source(D1, calendar)
            .expect("calendar source missing on fresh peer");
        let expected_email = format!("calendar-{i:02}@example.com");
        assert_eq!(source.account_id, format!("google-account-{i:02}"));
        assert_eq!(
            source.account_email.as_deref(),
            Some(expected_email.as_str())
        );
        assert_eq!(source.calendar_id, format!("calendar-id-{i:02}"));
        assert!(source.read_only);
    }
    for (date, daily) in daily_entries {
        assert_eq!(
            h.device(D1).workspace.daily_queue_scheme_id(date),
            Some(daily)
        );
        assert_eq!(h.device(D1).scheme_item_texts(daily), vec!["carry", "plan"]);
    }
}

#[test]
fn high_churn_four_device_randomized_stress_converges() {
    for seed in [11u64, 29, 97] {
        run_high_churn_seed(seed);
    }
}

fn run_high_churn_seed(seed: u64) {
    let mut rng = Rng::new(seed);
    let mut h = Harness::new(4);
    h.login_all();

    let mut schemes = Vec::new();
    for i in 0..16 {
        schemes.push(h.add_scheme(D0, &format!("stress-{i:02}"), &["seed"]));
    }
    h.settle();

    let devices = h.device_keys();
    for step in 0..420 {
        let device = devices[rng.below(devices.len() as u64) as usize];
        let scheme = schemes[rng.below(schemes.len() as u64) as usize];
        match rng.below(9) {
            0 | 1 => h.append_line(device, scheme, &format!("s{seed}-append-{step}")),
            2 => {
                let len = h.device(device).workspace.schemes[&scheme].items.len();
                if len > 0 {
                    h.edit_line(
                        device,
                        scheme,
                        rng.below(len as u64) as usize,
                        &format!("s{seed}-edit-{step}"),
                    );
                }
            }
            3 => {
                let len = h.device(device).workspace.schemes[&scheme].items.len();
                h.insert_line(
                    device,
                    scheme,
                    rng.below((len + 1) as u64) as usize,
                    &format!("s{seed}-insert-{step}"),
                );
            }
            4 => {
                let len = h.device(device).workspace.schemes[&scheme].items.len();
                if len > 2 {
                    h.remove_line(device, scheme, rng.below(len as u64) as usize);
                }
            }
            5 => h.reorder_reverse(device, scheme),
            6 => h.rename_scheme(device, scheme, &format!("stress-renamed-{seed}-{step}")),
            7 => toggle_archive(&mut h, device, scheme),
            _ => h.sync(device),
        }

        if step % 17 == 0 {
            h.sync(devices[rng.below(devices.len() as u64) as usize]);
        }
    }

    h.settle();
    h.assert_all_converged_with_context(seed);
    for device in devices {
        for scheme in &schemes {
            assert!(
                h.device(device).workspace.schemes.contains_key(scheme),
                "seed {seed}: {device:?} lost stress scheme {scheme}",
            );
        }
    }
}

fn toggle_archive(h: &mut Harness, device: DeviceKey, scheme: SchemeId) {
    if h.device(device).workspace.is_scheme_deleted(scheme) {
        h.restore_scheme(device, scheme);
    } else {
        h.archive_scheme(device, scheme);
    }
}
