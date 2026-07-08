use chrono::Local;
use gpui::prelude::*;
use gpui::{div, px, Context};
use knotq_l10n::{t as tr, t_with as tr_with};

use crate::app::KnotQApp;
use crate::theme_gpui::{token_hsla, token_rgba, Theme as UiTheme};

impl KnotQApp {
    pub(super) fn settings_sync_panel(&mut self, t: UiTheme, cx: &mut Context<Self>) -> gpui::AnyElement {
        let account = self.settings.sync_account.as_ref();
        let signed_in = account.is_some();
        let sync_enabled = account.is_some_and(|account| account.supports_sync);
        let status = account.and_then(|account| account.account_status.as_ref());
        let cancelled = status.is_some_and(|status| status.is_cancelled());
        let (badge, default_detail, badge_bg, badge_fg) =
            settings_sync_panel_state(signed_in, sync_enabled, cancelled, t);
        // Signed in, the subtitle identifies the account; the badge carries state.
        let detail = account
            .map(|account| account.email.clone())
            .unwrap_or(default_detail);
        // When cancelled, spell out the consequence (and the date access ends) right
        // under the email so it can't be mistaken for an active subscription.
        let cancel_notice =
            cancelled.then(
                || match status.and_then(|status| status.current_period_end) {
                    Some(end) => tr_with(
                        "settings.sync.cancelled_until",
                        &[(
                            "date",
                            &end.with_timezone(&Local).format("%b %-d, %Y").to_string(),
                        )],
                    ),
                    None => tr("settings.sync.cancelled_indefinite").to_string(),
                },
            );

        // Left block: the icon and the title/email, vertically centered together as
        // one unit over the full height of the card.
        let left_block = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(9.0))
            .child(settings_sync_glyph(t))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_size(px(16.0))
                            // Tight line height so the email sits directly beneath
                            // the title rather than below its leading.
                            .line_height(px(18.0))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(token_hsla(t.text_primary))
                            .child(tr("settings.sync.title")),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .text_size(px(12.0))
                            .line_height(px(15.0))
                            // The email is always one line; only the signed-out
                            // prompt needs two. Clamping to one avoids reserving an
                            // empty second line under it.
                            .line_clamp(if signed_in { 1 } else { 2 })
                            .text_color(token_hsla(t.text_soft))
                            .child(detail),
                    )
                    .when_some(cancel_notice, |column, notice| {
                        column.child(
                            div()
                                .min_w_0()
                                .text_size(px(12.0))
                                .line_height(px(16.0))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(token_hsla(if t.is_dark {
                                    0xf8d38dff
                                } else {
                                    0x9a4b00ff
                                }))
                                .child(notice),
                        )
                    }),
            );

        // Right block: status badge pinned to the top, account actions to the
        // bottom — unchanged from before, just gathered into one column so the left
        // block can center against its full height.
        let right_block = div()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .items_end()
            .justify_between()
            .gap(px(8.0))
            .child(
                div()
                    .flex_shrink_0()
                    .px(px(7.0))
                    .py(px(3.0))
                    .rounded(px(99.0))
                    .bg(token_rgba(badge_bg))
                    .text_size(px(11.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(token_hsla(badge_fg))
                    .child(badge),
            )
            .child(self.sync_account_management_section(t, cx));

        div()
            .w_full()
            .rounded(px(8.0))
            .border_1()
            .border_color(token_rgba(settings_sync_panel_border(t)))
            .bg(token_rgba(settings_sync_panel_bg(t)))
            .shadow_md()
            .p(px(12.0))
            .flex()
            .flex_row()
            .gap(px(9.0))
            .child(left_block)
            .child(right_block)
            .into_any_element()
    }
}

fn settings_sync_panel_state(
    signed_in: bool,
    sync_enabled: bool,
    cancelled: bool,
    t: UiTheme,
) -> (&'static str, String, u32, u32) {
    // Cancelled but still entitling: sync works until the period ends, so use the
    // amber "won't renew" treatment rather than the green active one.
    if sync_enabled && cancelled {
        return (
            tr("settings.sync.badge_cancelled"),
            tr("settings.sync.detail_cancelled").to_string(),
            if t.is_dark { 0xf59e0b28 } else { 0xd977061a },
            if t.is_dark { 0xf8d38dff } else { 0x9a4b00ff },
        );
    }

    if sync_enabled {
        return (
            tr("settings.sync.badge_subscribed"),
            tr("settings.sync.detail_subscribed").to_string(),
            if t.is_dark { 0x30d15826 } else { 0x1f8f4d18 },
            if t.is_dark { 0x9af0b6ff } else { 0x176b38ff },
        );
    }

    if signed_in {
        return (
            tr("settings.sync.badge_not_subscribed"),
            tr("settings.sync.detail_not_subscribed").to_string(),
            if t.is_dark { 0xf59e0b28 } else { 0xd977061a },
            if t.is_dark { 0xf8d38dff } else { 0x9a4b00ff },
        );
    }

    (
        tr("settings.sync.badge_available"),
        tr("settings.sync.detail_available").to_string(),
        if t.is_dark { 0x3b82f628 } else { 0x2f67cf18 },
        if t.is_dark { 0x9bc2ffff } else { 0x235ebeff },
    )
}

fn settings_sync_panel_bg(t: UiTheme) -> u32 {
    if t.is_dark {
        0x3b82f616
    } else {
        0xeaf2ffff
    }
}

fn settings_sync_panel_border(t: UiTheme) -> u32 {
    if t.is_dark {
        0x7aa0ff44
    } else {
        0x2f67cf38
    }
}

/// The brand mark: the actual KnotQ app icon, so the card is recognizably ours
/// rather than a generic glyph.
fn settings_sync_glyph(_t: UiTheme) -> gpui::AnyElement {
    div()
        .w(px(34.0))
        .h(px(34.0))
        .flex_shrink_0()
        .rounded(px(7.0))
        .overflow_hidden()
        .child(
            gpui::img("app-icon/128x128.png")
                .w(px(34.0))
                .h(px(34.0))
                .object_fit(gpui::ObjectFit::Cover),
        )
        .into_any_element()
}
