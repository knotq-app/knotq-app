use chrono::NaiveDate;
use knotq_model::RepeatEnd;

pub(crate) const UNTIL_CALENDAR_WIDTH: f32 = 220.0;
pub(crate) const UNTIL_CALENDAR_HEIGHT: f32 = 211.0;
pub(crate) const FOLDER_ICON_SIZE: f32 = 10.5;

pub(crate) fn repeat_end_for_local_date(date: NaiveDate) -> RepeatEnd {
    RepeatEnd::Until(
        knotq_date_util::local_date_repeat_until_utc(date)
            .expect("23:59:59 should resolve for a valid local calendar date"),
    )
}

pub mod calendar;
pub mod daily_queue;
pub mod date_popover;
pub mod editor_context_menu;
pub mod event_popup;
pub mod modals;
pub mod repeat_popover;
pub mod scheme_view;
pub mod search;
pub mod settings;
pub mod sidebar;
pub mod sync_account;
pub mod sync_popover;
pub mod title_bar;
pub mod upcoming;
