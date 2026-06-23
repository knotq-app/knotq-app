use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use std::collections::BTreeMap;
use std::path::Path;

use crate::capture::list_internal_snapshots;
use crate::store::write_manifest;
use crate::{InternalSnapshot, RetentionBucket, StoreManifest, STORE_VERSION};

pub(crate) fn rotate_snapshots(workspace_dir: &Path, now: DateTime<Utc>) -> Result<()> {
    let snapshots = list_internal_snapshots(workspace_dir)?;
    let mut refs = BTreeMap::<String, String>::new();
    let mut keep_by_ref = BTreeMap::<String, InternalSnapshot>::new();
    for snapshot in &snapshots {
        let Some(bucket) = retention_bucket(snapshot.timestamp, now) else {
            continue;
        };
        let refname = bucket.refname();
        let replace = keep_by_ref
            .get(&refname)
            .map(|existing| snapshot.timestamp > existing.timestamp)
            .unwrap_or(true);
        if replace {
            keep_by_ref.insert(refname, snapshot.clone());
        }
    }

    for (refname, snapshot) in keep_by_ref {
        refs.insert(refname, snapshot.id);
    }
    write_manifest(
        workspace_dir,
        &StoreManifest {
            version: STORE_VERSION,
            refs,
        },
    )?;
    Ok(())
}

pub(crate) fn retention_bucket(timestamp: DateTime<Utc>, now: DateTime<Utc>) -> Option<RetentionBucket> {
    let age = now.signed_duration_since(timestamp);
    let (tier, step_secs) = if age <= Duration::hours(1) {
        ("m5", 5 * 60)
    } else if age <= Duration::hours(48) {
        ("h1", 60 * 60)
    } else if age <= Duration::days(7) {
        ("h4", 4 * 60 * 60)
    } else if age <= Duration::days(365) {
        ("d1", 24 * 60 * 60)
    } else {
        return None;
    };
    Some(RetentionBucket {
        tier,
        start_epoch_secs: floor_epoch(timestamp.timestamp(), step_secs),
    })
}

fn floor_epoch(timestamp: i64, step_secs: i64) -> i64 {
    timestamp.div_euclid(step_secs) * step_secs
}
