//! Daily-queue carryover predicates, the notification-schedule stub, the
//! merged-state helper, and the deterministic test PRNG.
use super::*;

// ---------------------------------------------------------------------------
// Daily-queue carryover predicates
//
// Faithful copies of the private helpers in `knotq_state::daily_queue` (the source
// of truth for the "roll over from yesterday" action). They are duplicated here
// rather than imported because `knotq-state` depends on `knotq-sync`, so the sync
// crate cannot dev-depend on it without a cycle. Keep them in sync with
// `desktop/state/src/daily_queue.rs`.
// ---------------------------------------------------------------------------

/// How many days back the carryover scans for the most recent day with content.
const DQ_CARRYOVER_LOOKBACK_DAYS: i64 = 14;

pub(super) fn dq_last_nonblank_day(workspace: &Workspace, today: NaiveDate) -> Option<NaiveDate> {
    (1..=DQ_CARRYOVER_LOOKBACK_DAYS)
        .map(|offset| today - Duration::days(offset))
        .find(|date| {
            workspace
                .daily_queue_scheme_id(*date)
                .and_then(|id| workspace.scheme(id))
                .is_some_and(|scheme| !dq_scheme_is_blank(scheme))
        })
}

pub(super) fn dq_scheme_is_blank(scheme: &Scheme) -> bool {
    if scheme.items.is_empty() {
        return true;
    }
    scheme
        .items
        .first()
        .is_some_and(dq_item_is_blank_placeholder)
        && scheme.items.len() == 1
}

fn dq_item_is_blank_placeholder(item: &Item) -> bool {
    item.text().trim().is_empty()
        && !item.has_images()
        && !item.has_table()
        && item.marker == ItemMarker::Blank
        && item.indent == 0
        && !dq_item_has_annotations(item)
        && item.priority.is_none()
        && item.state.len() == 1
        && item.state[0].state.progress == 0
        && item.state[0].state.notification_offset_secs.is_none()
}

pub(super) fn dq_item_is_fully_complete_task(item: &Item) -> bool {
    item.marker == ItemMarker::Checkbox
        && !item.state.is_empty()
        && item.state.iter().all(|state| state.state.is_done())
}

pub(super) fn dq_item_has_annotations(item: &Item) -> bool {
    item.start.is_some() || item.end.is_some() || item.available.is_some() || item.repeats.is_some()
}

pub(super) fn dq_strip_annotations(item: &mut Item) {
    item.start = None;
    item.end = None;
    item.available = None;
    item.repeats = None;
}

pub(super) fn test_notification_schedule() -> NotificationScheduleSnapshot {
    let now = Utc::now();
    NotificationScheduleSnapshot {
        sequence: 0,
        // The real backend requires a 64-char sha256 hex hash and a non-empty
        // window (window_end > window_start).
        hash: "0".repeat(64),
        window_start: now,
        window_end: now + chrono::Duration::hours(1),
        occurrence_count: 0,
    }
}

/// Merge a stored merged state plus a batch of v1 updates into a new merged state,
/// exactly as the worker's `validateAndCompactCrdtUpdates` does.
pub(super) fn merge_state(base: &[u8], updates: &[Vec<u8>]) -> Vec<u8> {
    let doc = Doc::new();
    {
        let mut txn = doc.transact_mut();
        if !base.is_empty() {
            txn.apply_update(Update::decode_v1(base).expect("decode base"))
                .expect("apply base");
        }
        for update in updates {
            txn.apply_update(Update::decode_v1(update).expect("decode update"))
                .expect("apply update");
        }
    }
    let encoded = doc.transact().encode_diff_v1(&StateVector::default());
    encoded
}

/// Tiny deterministic PRNG (SplitMix64) so fuzz runs are reproducible.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed.wrapping_add(0x9E3779B97F4A7C15))
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }

    pub fn below(&mut self, bound: u64) -> u64 {
        if bound == 0 {
            0
        } else {
            self.next() % bound
        }
    }
}
