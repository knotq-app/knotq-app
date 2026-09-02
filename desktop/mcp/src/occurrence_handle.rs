//! Opaque round-tripping of an [`OccurrenceId`] through a single JSON string.
//!
//! A repeating item's occurrence identity is its *original* start, which can be
//! a floating date, a UTC instant, or a wall time in a named zone — the three
//! are not interchangeable, and collapsing them to an instant would silently
//! retarget a completion to the wrong day across a DST boundary. So the handle
//! is the serialized id itself rather than a formatted date: lossless by
//! construction, and it stays correct if the id ever grows a fourth variant.
//!
//! Agents are told to pass these back verbatim, never to build one.

use knotq_model::OccurrenceId;

/// The handle for a non-repeating item. Spelled out rather than JSON so the
/// overwhelmingly common case reads as something a human can recognise in a log.
const SINGLE: &str = "single";

pub fn encode(id: &OccurrenceId) -> String {
    match id {
        OccurrenceId::Single => SINGLE.to_string(),
        other => serde_json::to_string(other).unwrap_or_else(|_| SINGLE.to_string()),
    }
}

pub fn decode(handle: &str) -> Option<OccurrenceId> {
    let trimmed = handle.trim();
    if trimmed.is_empty() || trimmed == SINGLE {
        return Some(OccurrenceId::Single);
    }
    serde_json::from_str(trimmed).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use knotq_model::CalendarDateTime;

    #[test]
    fn single_round_trips_through_its_readable_spelling() {
        assert_eq!(encode(&OccurrenceId::Single), SINGLE);
        assert_eq!(decode(SINGLE), Some(OccurrenceId::Single));
    }

    #[test]
    fn an_absent_or_blank_handle_means_the_base_occurrence() {
        assert_eq!(decode(""), Some(OccurrenceId::Single));
        assert_eq!(decode("   "), Some(OccurrenceId::Single));
    }

    #[test]
    fn a_recurring_handle_round_trips_exactly() {
        let id = OccurrenceId::recurring_utc(Utc.with_ymd_and_hms(2026, 3, 8, 9, 30, 0).unwrap());
        assert_eq!(decode(&encode(&id)), Some(id));
    }

    #[test]
    fn a_zoned_occurrence_keeps_its_zone_rather_than_collapsing_to_an_instant() {
        // The whole reason the handle is the serialized id: 02:30 local on a
        // spring-forward day is not the same occurrence as any UTC instant, and
        // a lossy handle would complete a different day.
        let id = OccurrenceId::Recurring {
            original_start: CalendarDateTime::DateTimeWithZone {
                local: chrono::NaiveDate::from_ymd_opt(2026, 3, 8)
                    .unwrap()
                    .and_hms_opt(2, 30, 0)
                    .unwrap(),
                tzid: "America/New_York".to_string(),
            },
        };
        assert_eq!(decode(&encode(&id)), Some(id));
    }

    #[test]
    fn a_handle_the_agent_invented_is_rejected_rather_than_guessed_at() {
        assert_eq!(decode("2026-03-08T09:30:00Z"), None);
        assert_eq!(decode("{\"kind\":\"weekly\"}"), None);
    }
}
