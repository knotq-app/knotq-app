use chrono::{Datelike, Duration, Local, NaiveDate};
use gpui::{Context, Window};
use knotq_model::{ItemId, SavedView, SchemeId};
use knotq_storage_json::{CalendarViewMode, CalendarWeekRange};

use super::{add_months, KnotQApp, View};

impl KnotQApp {
    pub fn focus_app_root(&self, window: &mut Window) {
        self.editor_focus_handle.focus(window);
    }

    pub fn cancel_current_action(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.cal_drag.is_some() || self.cal_move.is_some() || self.cal_resize.is_some() {
            self.cal_drag = None;
            self.cal_move = None;
            self.cal_resize = None;
            cx.notify();
            return true;
        }
        if self.search_open {
            self.close_search(window, cx);
            return true;
        }
        if self.pending_delete.is_some() {
            self.cancel_delete_confirmation(cx);
            return true;
        }
        if self.rename_node.is_some() {
            self.cancel_new_node_prompt(cx);
            return true;
        }
        if self.event_popup.is_some() {
            if let Some(popup) = self.event_popup.as_mut() {
                if popup.scope_action.is_some() {
                    if popup.scope_dialog_only {
                        self.event_popup = None;
                        self.event_popup_title_subscription = None;
                        cx.notify();
                        return true;
                    }
                    popup.scope_action = None;
                    cx.notify();
                    return true;
                }
            }
            if self.cancel_event_popup_without_commit(cx) {
                self.focus_app_root(window);
                cx.notify();
                return true;
            }
        }
        if self.date_popover.is_some() {
            self.close_date_popover();
            self.focus_app_root(window);
            cx.notify();
            return true;
        }
        if self.repeat_popover.is_some() {
            self.close_repeat_popover();
            self.focus_app_root(window);
            cx.notify();
            return true;
        }
        false
    }

    pub fn open_union(&mut self) {
        self.selection.view = View::Union;
        self.dismiss_event_popup_if_hidden_context();
        self.persist_last_screen();
    }

    pub fn open_scheme(&mut self, scheme_id: SchemeId, focused_item: Option<ItemId>) {
        self.selection.view = View::Scheme;
        self.selection.scheme_id = Some(scheme_id);
        self.selection.focused_item_id = focused_item.or_else(|| {
            self.workspace
                .scheme(scheme_id)
                .and_then(|scheme| scheme.items.last())
                .map(|item| item.id)
        });
        self.dismiss_event_popup_if_hidden_context();
        self.persist_last_screen();
    }

    pub fn open_daily_queue(&mut self, cx: &mut Context<Self>) {
        self.sync_daily_queue_day_boundary(cx);
        self.selection.view = View::DailyQueue;
        let scheme_id = self.ensure_daily_queue_scheme(self.daily_queue_today, cx);
        self.selection.scheme_id = Some(scheme_id);
        self.selection.focused_item_id = None;
        self.daily_queue_loaded_start =
            super::daily_queue_default_window_start(self.daily_queue_today);
        self.daily_queue_preserved_bottom_distance = None;
        self.daily_queue_visible_dates.clear();
        self.daily_queue_scroll_initialized = false;
        self.dismiss_event_popup_if_hidden_context();
        self.persist_last_screen();
    }

    /// Reopen the screen saved from the previous session. A saved scheme that no
    /// longer exists (or was moved to the trash) falls back to the default Union
    /// view. Routes through the normal `open_*` methods so each view's setup
    /// (e.g. ensuring the daily-queue scheme) runs exactly as if navigated to.
    pub(crate) fn restore_last_screen(&mut self, cx: &mut Context<Self>) {
        match self.settings.last_view {
            Some(SavedView::Scheme) => {
                if let Some(id) = self.settings.last_scheme_id {
                    if self.workspace.scheme(id).is_some() && !self.workspace.is_scheme_deleted(id) {
                        self.open_scheme(id, None);
                    }
                }
            }
            Some(SavedView::DailyQueue) => self.open_daily_queue(cx),
            Some(SavedView::Union) | None => {}
        }
    }

    /// Record the current content view (and scheme, when in Scheme view) so the
    /// next launch reopens it. Settings is treated as transient — it keeps the
    /// prior content view saved rather than overwriting it.
    fn persist_last_screen(&mut self) {
        let saved = match self.selection.view {
            View::Union => SavedView::Union,
            View::DailyQueue => SavedView::DailyQueue,
            View::Scheme => SavedView::Scheme,
            View::Settings => return,
        };
        let scheme_id = (self.selection.view == View::Scheme)
            .then_some(self.selection.scheme_id)
            .flatten();
        if self.settings.last_view == Some(saved) && self.settings.last_scheme_id == scheme_id {
            return;
        }
        self.settings.last_view = Some(saved);
        self.settings.last_scheme_id = scheme_id;
        self.save_app_settings();
    }

    pub fn open_settings(&mut self, cx: &mut Context<Self>) {
        if self.selection.view == View::Settings {
            self.selection = self
                .settings_return_selection
                .take()
                .filter(|selection| selection.view != View::Settings)
                .unwrap_or_default();
        } else {
            self.settings_return_selection = Some(self.selection.clone());
            self.selection.view = View::Settings;
            self.refresh_account_status_quiet(cx);
        }
        self.dismiss_event_popup_if_hidden_context();
    }

    pub(crate) fn dismiss_event_popup_without_commit(&mut self) -> bool {
        self.close_date_popover();
        let dismissed = self.event_popup.take().is_some();
        if dismissed {
            self.event_popup_title_subscription = None;
        }
        dismissed
    }

    pub(crate) fn dismiss_event_popup_if_hidden_context(&mut self) -> bool {
        if event_popup_visible_in_context(self.selection.view, self.calendar_view) {
            return false;
        }
        self.dismiss_event_popup_without_commit()
    }

    pub fn shift_calendar_period(&mut self, delta: i32) {
        self.cal_swipe.offset_x = 0.0;
        match self.calendar_view {
            CalendarViewMode::Week => {
                self.week_offset += delta;
            }
            CalendarViewMode::Month => {
                self.month_offset += delta;
            }
        }
    }

    pub fn reset_calendar_period(&mut self) {
        self.cal_swipe.offset_x = 0.0;
        match self.calendar_view {
            CalendarViewMode::Week => self.week_offset = 0,
            CalendarViewMode::Month => self.month_offset = 0,
        }
    }

    pub fn calendar_week_start(&self) -> chrono::NaiveDate {
        calendar_week_start_for(
            Local::now().date_naive(),
            self.week_offset,
            self.calendar_week_range,
        )
    }

    pub fn calendar_month_start(&self) -> chrono::NaiveDate {
        add_months(Local::now().date_naive(), self.month_offset)
            .with_day(1)
            .unwrap_or_else(|| Local::now().date_naive())
    }
}

pub(crate) fn event_popup_visible_in_context(view: View, calendar_view: CalendarViewMode) -> bool {
    view == View::Union && calendar_view == CalendarViewMode::Week
}

fn calendar_week_start_for(
    today: NaiveDate,
    week_offset: i32,
    range: CalendarWeekRange,
) -> NaiveDate {
    match range {
        CalendarWeekRange::NextSevenDays => {
            today - Duration::days(1) + Duration::weeks(week_offset as i64)
        }
        CalendarWeekRange::CalendarWeek => {
            let dow = today.weekday().num_days_from_sunday() as i64;
            today - Duration::days(dow) + Duration::weeks(week_offset as i64)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rolling_week_starts_yesterday() {
        let wednesday = NaiveDate::from_ymd_opt(2026, 5, 20).unwrap();
        assert_eq!(
            calendar_week_start_for(wednesday, 0, CalendarWeekRange::NextSevenDays),
            NaiveDate::from_ymd_opt(2026, 5, 19).unwrap()
        );
        assert_eq!(
            calendar_week_start_for(wednesday, -1, CalendarWeekRange::NextSevenDays),
            NaiveDate::from_ymd_opt(2026, 5, 12).unwrap()
        );
        assert_eq!(
            calendar_week_start_for(wednesday, 1, CalendarWeekRange::NextSevenDays),
            NaiveDate::from_ymd_opt(2026, 5, 26).unwrap()
        );
    }

    #[test]
    fn calendar_week_range_starts_on_current_sunday() {
        let sunday = NaiveDate::from_ymd_opt(2026, 5, 17).unwrap();
        assert_eq!(
            calendar_week_start_for(sunday, 0, CalendarWeekRange::CalendarWeek),
            sunday
        );
        assert_eq!(
            calendar_week_start_for(sunday, -1, CalendarWeekRange::CalendarWeek),
            NaiveDate::from_ymd_opt(2026, 5, 10).unwrap()
        );
        assert_eq!(
            calendar_week_start_for(sunday, 1, CalendarWeekRange::CalendarWeek),
            NaiveDate::from_ymd_opt(2026, 5, 24).unwrap()
        );
    }

    #[test]
    fn calendar_week_range_rewinds_to_sunday_for_midweek_dates() {
        let wednesday = NaiveDate::from_ymd_opt(2026, 5, 20).unwrap();
        assert_eq!(
            calendar_week_start_for(wednesday, 0, CalendarWeekRange::CalendarWeek),
            NaiveDate::from_ymd_opt(2026, 5, 17).unwrap()
        );
    }

    #[test]
    fn event_popups_only_show_in_week_calendar_context() {
        assert!(event_popup_visible_in_context(
            View::Union,
            CalendarViewMode::Week
        ));
        assert!(!event_popup_visible_in_context(
            View::Union,
            CalendarViewMode::Month
        ));
        assert!(!event_popup_visible_in_context(
            View::Scheme,
            CalendarViewMode::Week
        ));
        assert!(!event_popup_visible_in_context(
            View::DailyQueue,
            CalendarViewMode::Week
        ));
        assert!(!event_popup_visible_in_context(
            View::Settings,
            CalendarViewMode::Week
        ));
    }
}
