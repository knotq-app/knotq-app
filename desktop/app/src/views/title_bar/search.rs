use gpui::prelude::*;
use gpui::{div, px, ClickEvent, Context, IntoElement, Window};
use knotq_l10n::t as tr;

use crate::app::KnotQApp;
use crate::theme_gpui::{token_hsla, token_rgba, Theme};

impl KnotQApp {
    pub(super) fn render_title_bar_search(
        &mut self,
        window: &mut Window,
        t: Theme,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        use gpui_component::input::Input;
        use gpui_component::Sizable as _;

        if self.search_open {
            let input = self.ensure_search_input(window, cx);
            div()
                .id("title-search")
                .h(px(26.0))
                .w(px(236.0))
                .pl(px(7.0))
                .pr(px(8.0))
                .rounded(px(7.0))
                .border_1()
                .border_color(token_rgba(t.border_soft))
                .bg(token_rgba(t.button_bg))
                .flex()
                .items_center()
                .gap(px(8.0))
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                    let input = this.ensure_search_input(window, cx);
                    input.update(cx, |input, cx| input.focus(window, cx));
                }))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .flex()
                        .items_center()
                        .child(
                            Input::new(&input)
                                .appearance(false)
                                .bordered(false)
                                .focus_bordered(false)
                                .xsmall()
                                .w_full()
                                .h_full(),
                        ),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(10.0))
                        .font_weight(gpui::FontWeight::NORMAL)
                        .text_color(token_hsla(t.text_muted))
                        .child("⌘F"),
                )
                .into_any_element()
        } else {
            div()
                .id("title-search")
                .h(px(26.0))
                .w(px(108.0))
                .px(px(8.0))
                .rounded(px(7.0))
                .border_1()
                .border_color(token_rgba(t.border_soft))
                .bg(token_rgba(t.button_bg))
                .flex()
                .items_center()
                .justify_between()
                .gap(px(10.0))
                .cursor_pointer()
                .hover({
                    let c = t.button_hover;
                    move |s| s.bg(token_rgba(c))
                })
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.open_search(window, cx);
                }))
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_weight(gpui::FontWeight::NORMAL)
                        .text_color(token_hsla(t.text_dim))
                        .child(tr("titlebar.search.placeholder")),
                )
                .child(
                    div()
                        .text_size(px(10.0))
                        .font_weight(gpui::FontWeight::NORMAL)
                        .text_color(token_hsla(t.text_muted))
                        .child("⌘F"),
                )
                .into_any_element()
        }
    }
}
