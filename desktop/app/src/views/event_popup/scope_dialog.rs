use super::*;

pub(super) fn scope_dialog(
    can_apply_this: bool,
    can_apply_future: bool,
    can_apply_all: bool,
    t: Theme,
    cx: &mut Context<KnotQApp>,
) -> gpui::AnyElement {
    div()
        .id("event-scope-dialog")
        .w(px(SCOPE_DIALOG_WIDTH))
        .rounded(px(8.0))
        .border_1()
        .border_color(token_rgba(t.border_overlay))
        .bg(token_hsla(t.bg_modal))
        .shadow_lg()
        .occlude()
        .overflow_hidden()
        .font_family(FONT_UI)
        .on_click(|_: &ClickEvent, _window, cx| cx.stop_propagation())
        .child(
            div()
                .px(px(16.0))
                .pt(px(14.0))
                .pb(px(10.0))
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(
                    div()
                        .text_size(px(14.0))
                        .line_height(px(18.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(token_hsla(t.text_primary))
                        .child(knotq_l10n::t("event.scope_dialog.title")),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .line_height(px(15.0))
                        .text_color(token_hsla(t.text_dim))
                        .child(knotq_l10n::t("event.scope_dialog.subtitle")),
                ),
        )
        .child(scope_dialog_row(
            "scope-this-event",
            knotq_l10n::t("event.scope_dialog.this_event"),
            can_apply_this,
            RepeatScope::ThisEvent,
            t,
            cx,
        ))
        .child(scope_dialog_row(
            "scope-future-events",
            knotq_l10n::t("event.scope_dialog.this_and_future"),
            can_apply_future,
            RepeatScope::AllFuture,
            t,
            cx,
        ))
        .child(scope_dialog_row(
            "scope-all-events",
            knotq_l10n::t("event.scope_dialog.all_events"),
            can_apply_all,
            RepeatScope::AllEvents,
            t,
            cx,
        ))
        .child(div().h(px(1.0)).mt(px(6.0)).bg(token_rgba(t.divider)))
        .child(
            div()
                .h(px(42.0))
                .px(px(12.0))
                .flex()
                .items_center()
                .justify_center()
                .child(cancel_scope_button(t, cx)),
        )
        .into_any_element()
}

pub(super) fn scope_dialog_row(
    id: &'static str,
    label: &'static str,
    enabled: bool,
    scope: RepeatScope,
    t: Theme,
    cx: &mut Context<KnotQApp>,
) -> gpui::AnyElement {
    div()
        .id(id)
        .h(px(32.0))
        .px(px(16.0))
        .flex()
        .items_center()
        .gap(px(7.0))
        .font_family(FONT_UI)
        .text_size(px(12.0))
        .cursor(if enabled {
            gpui::CursorStyle::PointingHand
        } else {
            gpui::CursorStyle::Arrow
        })
        .opacity(if enabled { 1.0 } else { 0.42 })
        .text_color(token_hsla(if enabled {
            t.text_primary
        } else {
            t.text_dim
        }))
        .hover({
            let hover = t.row_hover;
            move |s| {
                if enabled {
                    s.bg(token_rgba(hover))
                } else {
                    s
                }
            }
        })
        .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
            if enabled {
                this.apply_event_scope_choice(scope, cx);
            }
            cx.stop_propagation();
        }))
        .child(label)
        .into_any_element()
}

fn cancel_scope_button(t: Theme, cx: &mut Context<KnotQApp>) -> gpui::AnyElement {
    div()
        .id("scope-dialog-cancel")
        .px(px(10.0))
        .h(px(24.0))
        .rounded(px(4.0))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(11.0))
        .font_family(FONT_UI)
        .cursor_pointer()
        .text_color(token_hsla(t.text_primary))
        .hover({
            let hover = t.row_hover;
            move |s| s.bg(token_rgba(hover))
        })
        .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
            this.cancel_event_scope_dialog(cx);
            cx.stop_propagation();
        }))
        .child(knotq_l10n::t("common.cancel"))
        .into_any_element()
}
