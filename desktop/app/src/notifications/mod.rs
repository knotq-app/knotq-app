//! Cross-platform notification scheduling, action handling, reconciliation and
//! the durable notification manifest.
//!
//! The implementation is split by responsibility:
//! - [`common`] — shared identifiers, constants, manifest I/O, small helpers.
//! - [`setup`] — authorization requests and platform availability checks.
//! - [`compute`] — building the pending list and platform requests.
//! - [`reconcile`] — keeping the durable OS schedule in sync with that list.
//! - [`clearing`] — removing scheduled/delivered notifications.
//! - [`actions`] — handling user responses to delivered notifications.
mod actions;
mod clearing;
mod common;
mod compute;
mod reconcile;
mod setup;

pub use actions::*;
pub use clearing::*;
pub use common::*;
pub use compute::*;
pub use reconcile::*;
pub use setup::*;

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone, Utc};
    use knotq_model::{Item, ItemId, OccurrenceId, Scheme, SchemeId, Workspace};
    use knotq_notifications::{NotificationRequest, ScheduledNotification, ACTION_MARK_DONE};

    #[test]
    fn notification_request_has_stable_key() {
        let now = Utc::now();
        let note1 = NotificationRequest::new("stable-key", now, "T", "B");
        let note2 = NotificationRequest::new("stable-key", now, "T", "B");
        assert_eq!(note1.id, note2.id);
    }

    #[test]
    fn schedule_horizon_is_two_weeks() {
        assert_eq!(SCHEDULE_HORIZON_DAYS, 14);
    }

    #[test]
    fn notification_request_carries_expiration_metadata() {
        let fire_at = Utc.with_ymd_and_hms(2026, 5, 21, 12, 0, 0).unwrap();
        let expires_at = fire_at + Duration::hours(1);
        let note = ScheduledNotification {
            key: "key".to_string(),
            fire_at,
            expires_at: Some(expires_at),
            end_at: Some(expires_at),
            title: "Class".to_string(),
            body: "From Thu, 12:00 PM to 1:00 PM".to_string(),
            kind: knotq_notifications::NotificationKind::Event,
            trigger_at: fire_at,
            scheme_id: SchemeId::new(),
            item_id: ItemId::new(),
            occurrence: OccurrenceId::Single,
        };

        let request = notification_request(note);

        assert_eq!(request.expires_at, Some(expires_at));
        let expected_expires_at = expires_at.to_rfc3339();
        assert_eq!(
            request.user_info.get("expires_at").map(String::as_str),
            Some(expected_expires_at.as_str())
        );
        assert_eq!(
            request.user_info.get("end_at").map(String::as_str),
            Some(expected_expires_at.as_str())
        );
    }

    #[test]
    fn notification_target_resolves_stale_item_id_from_unique_occurrence() {
        let trigger_at = Utc.with_ymd_and_hms(2026, 5, 20, 9, 0, 0).unwrap();
        let item = Item::new("meeting").with_start(trigger_at);
        let item_id = item.id;
        let mut scheme = Scheme::new("Work", 0);
        let scheme_id = scheme.id;
        scheme.items.push(item);
        let mut workspace = Workspace::new();
        workspace.schemes.insert(scheme_id, scheme);

        let target = NotificationActionTarget {
            notification_id: "notification".to_string(),
            action_id: ACTION_MARK_DONE.to_string(),
            notification_key: Some(format!("{scheme_id}|single|r|{}", trigger_at.to_rfc3339())),
            scheme_id,
            item_id: ItemId::new(),
            occurrence: OccurrenceId::Single,
            trigger_at,
        };

        assert_eq!(
            resolve_notification_target_item_id(&workspace, &target),
            Some(item_id)
        );
    }

    #[test]
    fn notification_target_does_not_guess_when_stale_item_id_is_ambiguous() {
        let trigger_at = Utc.with_ymd_and_hms(2026, 5, 20, 9, 0, 0).unwrap();
        let mut scheme = Scheme::new("Work", 0);
        let scheme_id = scheme.id;
        scheme.items.push(Item::new("first").with_start(trigger_at));
        scheme
            .items
            .push(Item::new("second").with_start(trigger_at));
        let mut workspace = Workspace::new();
        workspace.schemes.insert(scheme_id, scheme);

        let target = NotificationActionTarget {
            notification_id: "notification".to_string(),
            action_id: ACTION_MARK_DONE.to_string(),
            notification_key: Some(format!("{scheme_id}|single|r|{}", trigger_at.to_rfc3339())),
            scheme_id,
            item_id: ItemId::new(),
            occurrence: OccurrenceId::Single,
            trigger_at,
        };

        assert_eq!(
            resolve_notification_target_item_id(&workspace, &target),
            None
        );
    }
}
