//! Seeded randomized fuzz scenario (l).

use chrono::{NaiveDate, TimeZone, Utc};
use knotq_model::ItemMarker;

use super::super::{Harness, Rng, D0};

// ---------------------------------------------------------------------------
// Scenario l — Seeded randomized fuzz (both backends)
// ---------------------------------------------------------------------------

/// A seeded 3-device randomized run with mixed ops including delete/archive/move/
/// daily-queue.  `op_count` scales for the backend: ~150 in-memory, ~60 over HTTP.
pub fn scenario_l_randomized_fuzz(h: &mut Harness, seed: u64, op_count: usize) {
    h.login_all();

    let mut rng = Rng::new(seed);
    let devices = h.device_keys();
    let n_devices = devices.len();

    // Seed schemes.
    let mut schemes = Vec::new();
    for i in 0..8 {
        schemes.push(h.add_scheme(D0, &format!("fuzz-{i}"), &["seed"]));
    }
    let folder = h.add_folder(D0, "FuzzFolder");
    h.settle();

    let dates: Vec<NaiveDate> = (1u32..=7)
        .map(|d| NaiveDate::from_ymd_opt(2026, 11, d).unwrap())
        .collect();

    for step in 0..op_count {
        let device = devices[rng.below(n_devices as u64) as usize];
        let scheme_idx = rng.below(schemes.len() as u64) as usize;
        let s = schemes[scheme_idx];

        match rng.below(14) {
            0 | 1 => h.append_line(device, s, &format!("f{seed}-s{step}")),
            2 => {
                let len = h
                    .device(device)
                    .workspace
                    .schemes
                    .get(&s)
                    .map(|sc| sc.items.len())
                    .unwrap_or(0);
                if len > 0 {
                    h.edit_line(
                        device,
                        s,
                        rng.below(len as u64) as usize,
                        &format!("f{seed}-e{step}"),
                    );
                }
            }
            3 => {
                let len = h
                    .device(device)
                    .workspace
                    .schemes
                    .get(&s)
                    .map(|sc| sc.items.len())
                    .unwrap_or(0);
                h.insert_line(
                    device,
                    s,
                    rng.below((len + 1) as u64) as usize,
                    &format!("f{seed}-i{step}"),
                );
            }
            4 => {
                let len = h
                    .device(device)
                    .workspace
                    .schemes
                    .get(&s)
                    .map(|sc| sc.items.len())
                    .unwrap_or(0);
                if len > 2 {
                    h.remove_line(device, s, rng.below(len as u64) as usize);
                }
            }
            5 => h.rename_scheme(device, s, &format!("fuzz-r-{seed}-{step}")),
            6 => {
                if h.device(device).workspace.is_scheme_deleted(s) {
                    h.restore_scheme(device, s);
                } else {
                    h.archive_scheme(device, s);
                }
            }
            7 => {
                h.move_scheme_to_folder(device, s, folder);
            }
            8 => {
                h.move_scheme_to_root(device, s);
            }
            9 => {
                let date = dates[rng.below(dates.len() as u64) as usize];
                h.set_daily_queue(device, date, &[&format!("fuzz-dq-{seed}-{step}")]);
            }
            10 => {
                // Item-level richness: change marker.
                let len = h
                    .device(device)
                    .workspace
                    .schemes
                    .get(&s)
                    .map(|sc| sc.items.len())
                    .unwrap_or(0);
                if len > 0 {
                    let idx = rng.below(len as u64) as usize;
                    let marker = match rng.below(4) {
                        0 => ItemMarker::Blank,
                        1 => ItemMarker::Bullet,
                        2 => ItemMarker::Numbered,
                        _ => ItemMarker::Checkbox,
                    };
                    h.set_item_marker(device, s, idx, marker);
                }
            }
            11 => {
                // Item-level richness: set dates.
                let len = h
                    .device(device)
                    .workspace
                    .schemes
                    .get(&s)
                    .map(|sc| sc.items.len())
                    .unwrap_or(0);
                if len > 0 {
                    let idx = rng.below(len as u64) as usize;
                    let start = Utc.with_ymd_and_hms(2026, 11, 1, 9, 0, 0).unwrap();
                    h.set_item_dates(device, s, idx, Some(start), None);
                }
            }
            12 => {
                // Item-level richness: change indent.
                let len = h
                    .device(device)
                    .workspace
                    .schemes
                    .get(&s)
                    .map(|sc| sc.items.len())
                    .unwrap_or(0);
                if len > 0 {
                    let idx = rng.below(len as u64) as usize;
                    h.set_item_indent(device, s, idx, (rng.below(4)) as u8);
                }
            }
            _ => h.sync(device),
        }

        if step % 13 == 0 {
            h.sync(devices[rng.below(n_devices as u64) as usize]);
        }
    }

    h.settle();
    h.assert_all_converged_with_context(seed);
}
