use gpui::ScrollHandle;

use super::super::*;

/// Convert a window-relative Y coordinate to an hour fraction (0.0–24.0)
/// using the calendar scroll container's bounds and offset.
pub(in crate::views::calendar) fn window_y_to_hour(window_y: f32, scroll_handle: &ScrollHandle) -> f32 {
    let bounds = scroll_handle.bounds();
    let viewport_y = window_y - f32::from(bounds.top());
    let scroll_offset_y = f32::from(scroll_handle.offset().y);
    let content_y = viewport_y - scroll_offset_y;
    ((content_y - TIME_Y_OFFSET) / HOUR_H).clamp(0.0, 24.0)
}

/// Resolve the target day for a move gesture from the cursor's horizontal
/// displacement relative to where the drag began. Using `round()` on the
/// displacement gives a half-column deadzone, so a mostly-vertical drag with a
/// little sideways wobble keeps the original day instead of randomly jumping to
/// an adjacent one. The result is clamped to the visible week.
pub(super) fn move_day_for_x(
    grab_x: f32,
    original_date: chrono::NaiveDate,
    window_x: f32,
    day_col_w: f32,
    visible_start: chrono::NaiveDate,
    visible_days: usize,
) -> chrono::NaiveDate {
    let day_delta = ((window_x - grab_x) / day_col_w.max(1.0)).round() as i64;
    let date = original_date + Duration::days(day_delta);
    let last = visible_start + Duration::days(visible_days as i64 - 1);
    date.clamp(visible_start, last)
}

pub(super) fn move_preview_hour(hour: f32) -> f32 {
    hour.clamp(0.0, 24.0)
}

/// Snap an absolute hour to the same 15-minute grid `snapped_calendar_datetime`
/// uses, so resize/create ghosts preview exactly where the edge will land. Keep
/// in sync with `knotq_date_util::snapped_calendar_datetime`.
pub(super) fn snap_preview_hour(hour: f32) -> f32 {
    let minutes = (hour * 60.0).round() as i64;
    let snapped = ((minutes + 7) / 15 * 15).clamp(0, 24 * 60);
    snapped as f32 / 60.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    const COL_W: f32 = 100.0;

    fn day_for(grab_x: f32, window_x: f32) -> NaiveDate {
        let start = NaiveDate::from_ymd_opt(2026, 5, 25).unwrap(); // Mon
        let original = NaiveDate::from_ymd_opt(2026, 5, 27).unwrap(); // Wed
        move_day_for_x(grab_x, original, window_x, COL_W, start, 7)
    }

    #[test]
    fn small_horizontal_wobble_keeps_the_original_day() {
        let original = NaiveDate::from_ymd_opt(2026, 5, 27).unwrap();
        // Anywhere within half a column of the grab point stays put.
        assert_eq!(day_for(350.0, 350.0), original);
        assert_eq!(day_for(350.0, 390.0), original); // +0.4 col
        assert_eq!(day_for(350.0, 310.0), original); // -0.4 col
    }

    #[test]
    fn moving_past_half_a_column_changes_the_day() {
        assert_eq!(
            day_for(350.0, 460.0), // +1.1 col → next day
            NaiveDate::from_ymd_opt(2026, 5, 28).unwrap()
        );
        assert_eq!(
            day_for(350.0, 240.0), // -1.1 col → previous day
            NaiveDate::from_ymd_opt(2026, 5, 26).unwrap()
        );
    }

    #[test]
    fn target_day_is_clamped_to_the_visible_week() {
        // Drag far left/right cannot escape the visible range.
        assert_eq!(
            day_for(350.0, -10_000.0),
            NaiveDate::from_ymd_opt(2026, 5, 25).unwrap()
        );
        assert_eq!(
            day_for(350.0, 10_000.0),
            NaiveDate::from_ymd_opt(2026, 5, 31).unwrap()
        );
    }
}
