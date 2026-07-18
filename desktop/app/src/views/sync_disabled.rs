//! UI stubs compiled when the `accounts` feature is off.
//!
//! All sign-in / sync / subscription surfaces are compiled out in this build. The
//! ungated render paths still call these methods, so provide fallbacks: the
//! Settings sync panel becomes a small "Coming soon" card, and the title-bar sync
//! control, status popover, and account-confirm modal render nothing.

use gpui::prelude::*;
use gpui::{div, px, Context, FontWeight, IntoElement, Window};

use crate::app::KnotQApp;
use crate::theme_gpui::{token_hsla, token_rgba, Theme};

impl KnotQApp {
    /// Replaces the Settings → Sync panel with a short "Coming soon" card.
    pub(crate) fn settings_sync_panel(&mut self, t: Theme, _cx: &mut Context<Self>) -> gpui::AnyElement {
        div()
            .w_full()
            .rounded(px(8.0))
            .border_1()
            .border_color(token_rgba(t.border_overlay))
            .bg(token_rgba(t.button_bg))
            .p(px(12.0))
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(
                div()
                    .text_size(px(13.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(token_hsla(t.text_primary))
                    .child("Coming soon"),
            )
            .child(
                div()
                    .text_size(px(12.0))
                    .line_height(px(18.0))
                    .text_color(token_hsla(t.text_soft))
                    .child("Cross-device sync and accounts are coming soon."),
            )
            .into_any_element()
    }

    /// No title-bar sync/enable control without account sync.
    pub(crate) fn render_title_bar_sync_control(
        &self,
        _t: Theme,
        _cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        None
    }

    /// No sync status popover without account sync.
    pub(crate) fn render_sync_status_popover(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        None
    }

    /// No account-action confirm modal without account sync.
    pub(crate) fn render_sync_account_confirm(
        &mut self,
        _cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        None
    }
}
