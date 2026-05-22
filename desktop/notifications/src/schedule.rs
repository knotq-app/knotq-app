use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration as StdDuration;

use crate::NotificationRequest;

pub const DEFAULT_DURABLE_NOTIFICATION_LIMIT: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReconciliationMode {
    Full,
    Targeted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlatformSchedulePolicy {
    pub os_pending_limit: usize,
    pub max_delivered_backlog: usize,
    pub max_schedule_horizon: Option<StdDuration>,
    pub add_interval: StdDuration,
    pub initial_verify_delay: StdDuration,
    pub retry_verify_delay: StdDuration,
}

impl PlatformSchedulePolicy {
    pub const fn new(os_pending_limit: usize) -> Self {
        Self {
            os_pending_limit,
            max_delivered_backlog: 32,
            max_schedule_horizon: None,
            add_interval: StdDuration::ZERO,
            initial_verify_delay: StdDuration::ZERO,
            retry_verify_delay: StdDuration::ZERO,
        }
    }

    pub const fn with_max_schedule_horizon(mut self, max_schedule_horizon: StdDuration) -> Self {
        self.max_schedule_horizon = Some(max_schedule_horizon);
        self
    }

    pub const fn with_add_interval(mut self, add_interval: StdDuration) -> Self {
        self.add_interval = add_interval;
        self
    }

    pub const fn with_verify_delays(
        mut self,
        initial_verify_delay: StdDuration,
        retry_verify_delay: StdDuration,
    ) -> Self {
        self.initial_verify_delay = initial_verify_delay;
        self.retry_verify_delay = retry_verify_delay;
        self
    }
}

#[derive(Clone, Debug)]
pub struct DurableNotificationSchedule {
    requests: Vec<NotificationRequest>,
}

impl DurableNotificationSchedule {
    pub fn new(
        requests: impl IntoIterator<Item = NotificationRequest>,
        now: DateTime<Utc>,
        limit: usize,
    ) -> Self {
        let requests = requests
            .into_iter()
            .filter(|request| request.fire_at > now)
            .take(limit)
            .collect();
        Self { requests }
    }

    pub fn requests(&self) -> &[NotificationRequest] {
        &self.requests
    }

    pub fn platform_window(&self, policy: PlatformSchedulePolicy) -> DesiredPlatformSchedule {
        let now = Utc::now();
        let cutoff = policy
            .max_schedule_horizon
            .and_then(|horizon| chrono::Duration::from_std(horizon).ok())
            .map(|horizon| now + horizon);
        DesiredPlatformSchedule::new(
            self.requests
                .iter()
                .filter(|request| cutoff.map_or(true, |cutoff| request.fire_at <= cutoff))
                .cloned()
                .take(policy.os_pending_limit),
            now,
        )
    }

    pub fn replace_manifest(&self, manifest: &mut ScheduleManifest) {
        manifest.replace_with_requests(&self.requests);
    }
}

#[derive(Clone, Debug)]
pub struct DesiredPlatformSchedule {
    requests: Vec<NotificationRequest>,
    ids: BTreeSet<String>,
    entries: BTreeMap<String, ScheduleManifestRequest>,
}

impl DesiredPlatformSchedule {
    pub fn new(
        requests: impl IntoIterator<Item = NotificationRequest>,
        now: DateTime<Utc>,
    ) -> Self {
        let requests = requests
            .into_iter()
            .filter(|request| request.fire_at > now)
            .collect::<Vec<_>>();
        let ids = requests
            .iter()
            .map(|request| request.id.clone())
            .collect::<BTreeSet<_>>();
        let entries = manifest_entries_for_requests(&requests);

        Self {
            requests,
            ids,
            entries,
        }
    }

    pub fn ids(&self) -> &BTreeSet<String> {
        &self.ids
    }

    pub fn requests_for(&self, ids: &BTreeSet<String>) -> Vec<NotificationRequest> {
        self.requests
            .iter()
            .filter(|request| ids.contains(&request.id))
            .cloned()
            .collect()
    }

    fn entries(&self) -> &BTreeMap<String, ScheduleManifestRequest> {
        &self.entries
    }
}

#[derive(Clone, Debug, Default)]
pub struct PlatformScheduleSnapshot {
    pending_managed: BTreeSet<String>,
    pending_legacy: BTreeSet<String>,
    delivered: BTreeSet<String>,
}

impl PlatformScheduleSnapshot {
    pub fn new(
        pending_ids: impl IntoIterator<Item = String>,
        delivered_ids: impl IntoIterator<Item = String>,
    ) -> Self {
        let mut pending_managed = BTreeSet::new();
        let mut pending_legacy = BTreeSet::new();
        for id in pending_ids {
            if is_managed_notification_id(&id) {
                pending_managed.insert(id);
            } else {
                pending_legacy.insert(id);
            }
        }

        Self {
            pending_managed,
            pending_legacy,
            delivered: delivered_ids.into_iter().collect(),
        }
    }

    pub fn pending_managed(&self) -> &BTreeSet<String> {
        &self.pending_managed
    }

    pub fn pending_legacy(&self) -> &BTreeSet<String> {
        &self.pending_legacy
    }

    pub fn delivered(&self) -> &BTreeSet<String> {
        &self.delivered
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScheduleReconciliationPlan {
    pub to_cancel: BTreeSet<String>,
    pub to_schedule: BTreeSet<String>,
    pub kept_count: usize,
    pub desired_count: usize,
}

impl ScheduleReconciliationPlan {
    pub fn new(
        snapshot: &PlatformScheduleSnapshot,
        desired: &DesiredPlatformSchedule,
        manifest: &ScheduleManifest,
        mode: ReconciliationMode,
    ) -> Self {
        let changed = desired
            .ids()
            .intersection(snapshot.pending_managed())
            .filter(|id| manifest.requests.get(*id) != desired.entries().get(*id))
            .cloned()
            .collect::<BTreeSet<_>>();
        let missing = desired
            .ids()
            .difference(snapshot.pending_managed())
            .cloned()
            .collect::<BTreeSet<_>>();

        let mut to_cancel = changed.clone();
        if mode == ReconciliationMode::Full {
            to_cancel.extend(
                snapshot
                    .pending_managed()
                    .difference(desired.ids())
                    .cloned(),
            );
        }

        let mut to_schedule = missing;
        to_schedule.extend(changed);

        Self {
            kept_count: desired.ids().len().saturating_sub(to_schedule.len()),
            desired_count: desired.ids().len(),
            to_cancel,
            to_schedule,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RetentionReport {
    pub retained_count: usize,
    pub desired_count: usize,
    pub missing: Vec<String>,
    pub stale: Vec<String>,
}

impl RetentionReport {
    pub fn new(snapshot: &PlatformScheduleSnapshot, desired: &DesiredPlatformSchedule) -> Self {
        let missing = desired
            .ids()
            .difference(snapshot.pending_managed())
            .cloned()
            .collect::<Vec<_>>();
        let stale = snapshot
            .pending_managed()
            .difference(desired.ids())
            .cloned()
            .collect::<Vec<_>>();

        Self {
            retained_count: snapshot
                .pending_managed()
                .intersection(desired.ids())
                .count(),
            desired_count: desired.ids().len(),
            missing,
            stale,
        }
    }

    pub fn is_complete(&self) -> bool {
        self.missing.is_empty()
    }

    pub fn missing_preview(&self, max: usize) -> String {
        self.missing
            .iter()
            .take(max)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ScheduleManifest {
    pub requests: BTreeMap<String, ScheduleManifestRequest>,
}

impl ScheduleManifest {
    pub fn replace_with_requests(&mut self, requests: &[NotificationRequest]) {
        self.requests = manifest_entries_for_requests(requests);
    }

    pub fn update_requests(&mut self, requests: &[NotificationRequest]) {
        for request in requests {
            self.requests
                .insert(request.id.clone(), schedule_manifest_entry(request));
        }
    }

    pub fn prune_expired(&mut self, now: DateTime<Utc>) -> Vec<String> {
        let mut expired = Vec::new();
        self.requests.retain(|id, request| {
            if request
                .expires_at
                .is_some_and(|expires_at| expires_at <= now)
            {
                expired.push(id.clone());
                false
            } else {
                true
            }
        });
        expired
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ScheduleManifestRequest {
    pub fingerprint: String,
    pub expires_at: Option<DateTime<Utc>>,
}

pub fn delivered_cleanup_ids(
    candidates: impl IntoIterator<Item = String>,
    delivered: &BTreeSet<String>,
) -> Vec<String> {
    let mut ids = candidates
        .into_iter()
        .filter(|id| delivered.contains(id))
        .collect::<Vec<_>>();
    ids.sort();
    ids.dedup();
    ids
}

pub fn delivered_backlog_exceeds(snapshot: &PlatformScheduleSnapshot, limit: usize) -> bool {
    snapshot.delivered().len() > limit
}

pub fn is_managed_notification_id(id: &str) -> bool {
    let Some(hex) = id.strip_prefix("knotq-") else {
        return false;
    };
    hex.len() == 16 && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub fn schedule_manifest_entry(request: &NotificationRequest) -> ScheduleManifestRequest {
    ScheduleManifestRequest {
        fingerprint: request_fingerprint(request),
        expires_at: request.expires_at,
    }
}

fn manifest_entries_for_requests(
    requests: &[NotificationRequest],
) -> BTreeMap<String, ScheduleManifestRequest> {
    requests
        .iter()
        .map(|request| (request.id.clone(), schedule_manifest_entry(request)))
        .collect()
}

fn request_fingerprint(request: &NotificationRequest) -> String {
    let mut hasher = Sha256::new();
    hasher.update(request.id.as_bytes());
    hasher.update([0]);
    hasher.update(request.fire_at.to_rfc3339().as_bytes());
    hasher.update([0]);
    if let Some(expires_at) = request.expires_at {
        hasher.update(expires_at.to_rfc3339().as_bytes());
    }
    hasher.update([0]);
    hasher.update(request.title.as_bytes());
    hasher.update([0]);
    hasher.update(request.body.as_bytes());
    hasher.update([0]);
    if let Some(group) = &request.group {
        hasher.update(group.as_bytes());
    }
    hasher.update([0]);
    if let Some(category) = &request.category {
        hasher.update(category.as_bytes());
    }
    for (key, value) in &request.user_info {
        hasher.update([0]);
        hasher.update(key.as_bytes());
        hasher.update([0]);
        hasher.update(value.as_bytes());
    }
    let digest = hasher.finalize();
    format!(
        "{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        digest[0], digest[1], digest[2], digest[3], digest[4], digest[5], digest[6], digest[7]
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn request(id: &str, title: &str) -> NotificationRequest {
        NotificationRequest::new(id, Utc::now() + Duration::hours(1), title, "body")
    }

    fn manifest_entry(fingerprint: &str) -> ScheduleManifestRequest {
        ScheduleManifestRequest {
            fingerprint: fingerprint.to_string(),
            expires_at: None,
        }
    }

    #[test]
    fn managed_notification_id_matches_only_hashed_knotq_ids() {
        assert!(is_managed_notification_id("knotq-0123456789abcdef"));
        assert!(!is_managed_notification_id("knotq-test-scheduled-123"));
        assert!(!is_managed_notification_id("other-0123456789abcdef"));
    }

    #[test]
    fn request_fingerprint_changes_when_content_changes() {
        let first = request("knotq-0123456789abcdef", "T");
        let second = request("knotq-0123456789abcdef", "T2");
        assert_ne!(
            schedule_manifest_entry(&first).fingerprint,
            schedule_manifest_entry(&second).fingerprint
        );
    }

    #[test]
    fn durable_schedule_keeps_next_requests_separate_from_platform_window() {
        let now = Utc::now();
        let requests = (0..80)
            .map(|idx| {
                NotificationRequest::new(
                    format!("knotq-{idx:016x}"),
                    now + Duration::minutes(idx + 1),
                    "T",
                    "B",
                )
            })
            .collect::<Vec<_>>();

        let durable =
            DurableNotificationSchedule::new(requests, now, DEFAULT_DURABLE_NOTIFICATION_LIMIT);
        let desired = durable.platform_window(PlatformSchedulePolicy::new(16));

        assert_eq!(durable.requests().len(), 64);
        assert_eq!(desired.ids().len(), 16);
    }

    #[test]
    fn platform_window_skips_requests_beyond_hard_horizon_before_limit() {
        let now = Utc::now();
        let mut requests = (0..20)
            .map(|idx| {
                NotificationRequest::new(
                    format!("knotq-{idx:016x}"),
                    now + Duration::days(33) + Duration::minutes(idx),
                    "T",
                    "B",
                )
            })
            .collect::<Vec<_>>();
        requests.push(NotificationRequest::new(
            "knotq-ffffffffffffffff",
            now + Duration::days(2),
            "near",
            "body",
        ));

        let durable =
            DurableNotificationSchedule::new(requests, now, DEFAULT_DURABLE_NOTIFICATION_LIMIT);
        let desired = durable.platform_window(
            PlatformSchedulePolicy::new(16)
                .with_max_schedule_horizon(StdDuration::from_secs(32 * 24 * 60 * 60)),
        );

        assert_eq!(desired.ids().len(), 1);
        assert!(desired.ids().contains("knotq-ffffffffffffffff"));
    }

    #[test]
    fn full_reconciliation_keeps_changed_missing_and_stale_separate() {
        let unchanged = "knotq-0000000000000001".to_string();
        let changed = "knotq-0000000000000002".to_string();
        let missing = "knotq-0000000000000003".to_string();
        let stale = "knotq-0000000000000004".to_string();

        let snapshot = PlatformScheduleSnapshot::new(
            [unchanged.clone(), changed.clone(), stale.clone()],
            Vec::<String>::new(),
        );
        let desired = DesiredPlatformSchedule {
            requests: Vec::new(),
            ids: BTreeSet::from([unchanged.clone(), changed.clone(), missing.clone()]),
            entries: BTreeMap::from([
                (unchanged.clone(), manifest_entry("same")),
                (changed.clone(), manifest_entry("new")),
                (missing.clone(), manifest_entry("new")),
            ]),
        };
        let manifest = ScheduleManifest {
            requests: BTreeMap::from([
                (unchanged.clone(), manifest_entry("same")),
                (changed.clone(), manifest_entry("old")),
            ]),
        };

        let plan = ScheduleReconciliationPlan::new(
            &snapshot,
            &desired,
            &manifest,
            ReconciliationMode::Full,
        );

        assert_eq!(plan.to_cancel, BTreeSet::from([changed.clone(), stale]));
        assert_eq!(plan.to_schedule, BTreeSet::from([changed, missing]));
        assert_eq!(plan.kept_count, 1);
        assert_eq!(plan.desired_count, 3);
    }

    #[test]
    fn targeted_reconciliation_does_not_reschedule_unchanged_pending_request() {
        let unchanged = "knotq-0000000000000001".to_string();
        let snapshot = PlatformScheduleSnapshot::new([unchanged.clone()], Vec::<String>::new());
        let desired = DesiredPlatformSchedule {
            requests: Vec::new(),
            ids: BTreeSet::from([unchanged.clone()]),
            entries: BTreeMap::from([(unchanged.clone(), manifest_entry("same"))]),
        };
        let manifest = ScheduleManifest {
            requests: BTreeMap::from([(unchanged, manifest_entry("same"))]),
        };

        let plan = ScheduleReconciliationPlan::new(
            &snapshot,
            &desired,
            &manifest,
            ReconciliationMode::Targeted,
        );

        assert!(plan.to_cancel.is_empty());
        assert!(plan.to_schedule.is_empty());
        assert_eq!(plan.kept_count, 1);
        assert_eq!(plan.desired_count, 1);
    }

    #[test]
    fn targeted_reconciliation_reschedules_changed_but_not_stale_other_items() {
        let changed = "knotq-0000000000000001".to_string();
        let stale_other_item = "knotq-0000000000000002".to_string();
        let snapshot = PlatformScheduleSnapshot::new(
            [changed.clone(), stale_other_item.clone()],
            Vec::<String>::new(),
        );
        let desired = DesiredPlatformSchedule {
            requests: Vec::new(),
            ids: BTreeSet::from([changed.clone()]),
            entries: BTreeMap::from([(changed.clone(), manifest_entry("new"))]),
        };
        let manifest = ScheduleManifest {
            requests: BTreeMap::from([(changed.clone(), manifest_entry("old"))]),
        };

        let plan = ScheduleReconciliationPlan::new(
            &snapshot,
            &desired,
            &manifest,
            ReconciliationMode::Targeted,
        );

        assert_eq!(plan.to_cancel, BTreeSet::from([changed.clone()]));
        assert_eq!(plan.to_schedule, BTreeSet::from([changed]));
        assert!(!plan.to_cancel.contains(&stale_other_item));
    }

    #[test]
    fn delivered_cleanup_intersects_candidates_with_actual_delivered_ids() {
        let delivered = BTreeSet::from(["a".to_string(), "c".to_string()]);
        let cleanup = delivered_cleanup_ids(["a".to_string(), "b".to_string()], &delivered);

        assert_eq!(cleanup, vec!["a".to_string()]);
    }
}
