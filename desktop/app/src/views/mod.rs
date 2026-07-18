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
#[cfg(feature = "accounts")]
pub mod sync_account;
#[cfg(not(feature = "accounts"))]
mod sync_disabled;
#[cfg(feature = "accounts")]
pub mod sync_popover;
pub mod title_bar;
pub mod upcoming;

/// Accent CTA colors, shared by the auto-update pill, settings action rows, and
/// (under `accounts`) the sync sign-in buttons. Defined here (not in the
/// account-gated `sync_account` module) so the ungated auto-update UI can use them
/// even when account sync is compiled out.
pub(crate) fn sync_cta_bg() -> u32 {
    0x2563ebff
}

pub(crate) fn sync_cta_hover_bg() -> u32 {
    0x1d4ed8ff
}
