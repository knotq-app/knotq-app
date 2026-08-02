//! How often a workspace history snapshot is actually worth taking.
//!
//! `record_workspace_snapshot` runs after every workspace save — which means
//! after every pause in typing — and each call re-reads and re-hashes every
//! tracked file before it can even decide the content is unchanged. On mobile
//! that was ~13 ms of a ~40 ms edit, spent almost entirely on snapshots that are
//! then discarded: the finest retention tier is 5 minutes (see `retention.rs`),
//! so at most one snapshot per 5-minute window survives rotation.
//!
//! Throttling to a minute keeps restore points at a granularity the retention
//! policy can actually represent, for ~1/60th of the cost. The workspace itself
//! is still written on every edit — this is the restore-point history, not the
//! durability path.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use chrono::{DateTime, Duration, Utc};

/// Minimum gap between snapshots of the same workspace.
pub(crate) fn min_snapshot_interval() -> Duration {
    Duration::minutes(1)
}

/// Remembers when each workspace last had a snapshot taken.
#[derive(Debug, Default)]
pub(crate) struct SnapshotCadence {
    last: HashMap<PathBuf, DateTime<Utc>>,
}

impl SnapshotCadence {
    /// Whether a snapshot is due for `workspace_dir` now.
    ///
    /// The first call for a workspace always records, so a freshly launched app
    /// gets a restore point straight away rather than after a minute of edits.
    pub(crate) fn is_due(
        &self,
        workspace_dir: &Path,
        now: DateTime<Utc>,
        min_interval: Duration,
    ) -> bool {
        match self.last.get(workspace_dir) {
            Some(&last) => now < last || now.signed_duration_since(last) >= min_interval,
            None => true,
        }
    }

    /// Records a snapshot that was successfully written. This is deliberately
    /// separate from `is_due`: a transient filesystem failure must retry on the
    /// next save rather than silently suppress history for a full interval.
    pub(crate) fn recorded(
        &mut self,
        workspace_dir: &Path,
        now: DateTime<Utc>,
    ) {
        self.last.insert(workspace_dir.to_path_buf(), now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(minute: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(minute * 60, 0).unwrap()
    }

    #[test]
    fn first_snapshot_for_a_workspace_is_always_taken() {
        let cadence = SnapshotCadence::default();
        assert!(cadence.is_due(Path::new("/a"), at(0), Duration::minutes(1)));
    }

    #[test]
    fn a_second_snapshot_inside_the_interval_is_skipped() {
        let mut cadence = SnapshotCadence::default();
        let interval = Duration::minutes(1);
        cadence.recorded(Path::new("/a"), at(0));
        assert!(!cadence.is_due(Path::new("/a"), at(0) + Duration::seconds(59), interval));
    }

    #[test]
    fn a_snapshot_after_the_interval_is_taken() {
        let mut cadence = SnapshotCadence::default();
        let interval = Duration::minutes(1);
        cadence.recorded(Path::new("/a"), at(0));
        assert!(cadence.is_due(Path::new("/a"), at(1), interval));
        // …and that one resets the window rather than letting the next slip through.
        cadence.recorded(Path::new("/a"), at(1));
        assert!(!cadence.is_due(Path::new("/a"), at(1) + Duration::seconds(30), interval));
    }

    #[test]
    fn workspaces_are_throttled_independently() {
        let mut cadence = SnapshotCadence::default();
        let interval = Duration::minutes(1);
        cadence.recorded(Path::new("/a"), at(0));
        assert!(
            cadence.is_due(Path::new("/b"), at(0), interval),
            "one workspace's snapshot must not suppress another's"
        );
    }

    /// A backwards clock jump must not wedge snapshots off for however long the
    /// jump was.
    #[test]
    fn a_clock_moving_backwards_does_not_block_snapshots() {
        let mut cadence = SnapshotCadence::default();
        let interval = Duration::minutes(1);
        cadence.recorded(Path::new("/a"), at(100));
        assert!(cadence.is_due(Path::new("/a"), at(10), interval));
        // The earlier time is now the reference point.
        cadence.recorded(Path::new("/a"), at(10));
        assert!(!cadence.is_due(Path::new("/a"), at(10) + Duration::seconds(5), interval));
    }

    #[test]
    fn a_failed_snapshot_does_not_consume_the_interval() {
        let cadence = SnapshotCadence::default();
        assert!(cadence.is_due(Path::new("/a"), at(0), Duration::minutes(1)));
        assert!(cadence.is_due(Path::new("/a"), at(0), Duration::minutes(1)));
    }
}
