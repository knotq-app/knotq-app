//! Hard CRDT scenarios executed against the in-memory [`TestServer`].
//!
//! Each test is a thin wrapper calling the shared scenario function from
//! `common::scenarios`.  The HTTP equivalents live in `backend_integration.rs`.

mod common;

use common::scenarios;
use common::Harness;

// ---------------------------------------------------------------------------
// Scenario a — Edit-vs-delete race
// ---------------------------------------------------------------------------

#[test]
fn scenario_a_edit_vs_delete_a_syncs_first() {
    let mut h = Harness::new(2);
    scenarios::scenario_a_edit_vs_delete_a_first(&mut h);
}

#[test]
fn scenario_a_edit_vs_delete_b_syncs_first() {
    let mut h = Harness::new(2);
    scenarios::scenario_a_edit_vs_delete_b_first(&mut h);
}

// ---------------------------------------------------------------------------
// Scenario b — Delete-vs-archive race
// ---------------------------------------------------------------------------

#[test]
fn scenario_b_delete_vs_archive_race() {
    let mut h = Harness::new(2);
    scenarios::scenario_b_delete_vs_archive_race(&mut h);
}

// ---------------------------------------------------------------------------
// Scenario c — Folder shuffle storm
// ---------------------------------------------------------------------------

#[test]
fn scenario_c_folder_shuffle_storm() {
    let mut h = Harness::new(2);
    scenarios::scenario_c_folder_shuffle_storm(&mut h);
}

// ---------------------------------------------------------------------------
// Scenario d — Zig-zag workspace/document interleave
// ---------------------------------------------------------------------------

#[test]
fn scenario_d_zigzag_interleave() {
    let mut h = Harness::new(2);
    scenarios::scenario_d_zigzag_interleave(&mut h);
}

// ---------------------------------------------------------------------------
// Scenario e — Long offline divergence
// ---------------------------------------------------------------------------

#[test]
fn scenario_e_long_offline_divergence() {
    let mut h = Harness::new(2);
    scenarios::scenario_e_long_offline_divergence(&mut h);
}

// ---------------------------------------------------------------------------
// Scenario f — Offline + multiple restarts
// ---------------------------------------------------------------------------

#[test]
fn scenario_f_offline_restart_combo() {
    let mut h = Harness::new(2);
    scenarios::scenario_f_offline_restart_combo(&mut h);
}

// ---------------------------------------------------------------------------
// Scenario g — Daily queue conflicts
// ---------------------------------------------------------------------------

#[test]
fn scenario_g_daily_queue_conflicts() {
    let mut h = Harness::new(2);
    scenarios::scenario_g_daily_queue_conflicts(&mut h);
}

#[test]
fn scenario_g2_daily_queue_direct_creation() {
    let mut h = Harness::new(2);
    scenarios::scenario_g2_daily_queue_direct_creation(&mut h);
}

// ---------------------------------------------------------------------------
// Scenario h — Calendar import lifecycle
// ---------------------------------------------------------------------------

#[test]
fn scenario_h_calendar_import_lifecycle() {
    let mut h = Harness::new(2);
    scenarios::scenario_h_calendar_import_lifecycle(&mut h);
}

// ---------------------------------------------------------------------------
// Scenario i — Media variants
// ---------------------------------------------------------------------------

#[test]
fn scenario_i_media_concurrent_attach() {
    let mut h = Harness::new(2);
    scenarios::scenario_i_media_variants(&mut h);
}

#[test]
fn scenario_i_media_scheme_deleted() {
    let mut h = Harness::new(2);
    scenarios::scenario_i_media_scheme_deleted(&mut h);
}

// ---------------------------------------------------------------------------
// Scenario j — Notification schedule interleaved with doc edits
// ---------------------------------------------------------------------------

#[test]
fn scenario_j_notification_schedule() {
    let mut h = Harness::new(2);
    scenarios::scenario_j_notification_schedule(&mut h);
}

// ---------------------------------------------------------------------------
// Scenario k — Fresh device join mid-chaos (3 devices)
// ---------------------------------------------------------------------------

#[test]
fn scenario_k_fresh_device_join() {
    let mut h = Harness::new(3);
    scenarios::scenario_k_fresh_device_join(&mut h);
}

// ---------------------------------------------------------------------------
// Scenario l — Seeded randomized fuzz
// ---------------------------------------------------------------------------

#[test]
fn scenario_l_randomized_fuzz_seeds() {
    // Run several fixed seeds in-memory at full op count.
    for seed in [42u64, 137, 271, 999] {
        let mut h = Harness::new(3);
        scenarios::scenario_l_randomized_fuzz(&mut h, seed, 150);
    }
}

// ---------------------------------------------------------------------------
// Scenario m / n — Daily-queue "roll over from yesterday" (carryover) family
// ---------------------------------------------------------------------------

#[test]
fn scenario_m_carryover_basic() {
    let mut h = Harness::new(2);
    scenarios::scenario_m_carryover_basic(&mut h);
}

#[test]
fn scenario_m2_carryover_concurrent_shared_today() {
    let mut h = Harness::new(2);
    scenarios::scenario_m2_carryover_concurrent_shared_today(&mut h);
}

#[test]
fn scenario_m3_carryover_concurrent_independent_today() {
    let mut h = Harness::new(2);
    scenarios::scenario_m3_carryover_concurrent_independent_today(&mut h);
}

#[test]
fn scenario_m4_carryover_vs_yesterday_edit() {
    let mut h = Harness::new(2);
    scenarios::scenario_m4_carryover_vs_yesterday_edit(&mut h);
}

#[test]
fn scenario_m5_carryover_offline_restart() {
    let mut h = Harness::new(2);
    scenarios::scenario_m5_carryover_offline_restart(&mut h);
}

#[test]
fn scenario_n_carryover_chain() {
    let mut h = Harness::new(2);
    scenarios::scenario_n_carryover_chain(&mut h);
}
