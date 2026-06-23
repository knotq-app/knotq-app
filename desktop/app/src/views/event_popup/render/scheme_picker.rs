use super::super::*;

impl KnotQApp {
    pub(super) fn render_event_scheme_picker(
        &self,
        current_scheme_id: SchemeId,
        left: Pixels,
        top: Pixels,
        t: Theme,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let mut rows = Vec::new();
        let today_daily_id = self.workspace.daily_queue_scheme_id(self.daily_queue_today);
        rows.push(event_scheme_picker_daily_row(
            today_daily_id == Some(current_scheme_id),
            t,
            cx,
        ));
        rows.extend(self.render_event_scheme_picker_children(
            self.workspace.root,
            0,
            current_scheme_id,
            t,
            cx,
        ));

        div()
            .id("popup-scheme-picker")
            .absolute()
            .left(left)
            .top(top)
            .w(px(SCHEME_PICKER_WIDTH))
            .max_h(px(260.0))
            .overflow_y_scroll()
            .rounded(px(7.0))
            .border_1()
            .border_color(token_rgba(t.border_overlay))
            .bg(token_hsla(t.bg_modal))
            .shadow_lg()
            .occlude()
            .px(px(6.0))
            .py(px(6.0))
            .flex()
            .flex_col()
            .gap(px(1.0))
            .on_click(|_: &ClickEvent, _window, cx| cx.stop_propagation())
            .children(rows)
            .into_any_element()
    }

    fn render_event_scheme_picker_children(
        &self,
        folder_id: FolderId,
        depth: usize,
        current_scheme_id: SchemeId,
        t: Theme,
        cx: &mut Context<Self>,
    ) -> Vec<gpui::AnyElement> {
        let children = self
            .workspace
            .folder(folder_id)
            .map(|folder| folder.children.clone())
            .unwrap_or_default();
        let mut rows = Vec::new();
        for child in children {
            match child {
                NodeRef::Folder(id) => {
                    let Some(folder) = self.workspace.folder(id) else {
                        continue;
                    };
                    rows.push(event_scheme_picker_folder_row(&folder.name, id, depth, t));
                    rows.extend(self.render_event_scheme_picker_children(
                        id,
                        depth + 1,
                        current_scheme_id,
                        t,
                        cx,
                    ));
                }
                NodeRef::Scheme(id) => {
                    let Some(scheme) = self.workspace.scheme(id) else {
                        continue;
                    };
                    if scheme.is_read_only() || self.workspace.is_daily_queue_scheme(id) {
                        continue;
                    }
                    rows.push(event_scheme_picker_scheme_row(
                        id,
                        scheme.name.clone(),
                        scheme.color_index,
                        depth,
                        id == current_scheme_id,
                        t,
                        cx,
                    ));
                }
            }
        }
        rows
    }
}

fn event_scheme_picker_daily_row(
    selected: bool,
    t: Theme,
    cx: &mut Context<KnotQApp>,
) -> gpui::AnyElement {
    let row_color = token_hsla(daily_queue_marker_color(t.is_dark));
    let row_bg = scheme_picker_row_bg(row_color, selected, t);
    let row_hover = scheme_picker_row_hover(row_color, selected, t);
    let row_border = scheme_picker_row_border(row_color, selected, t);

    div()
        .id("popup-scheme-pick-daily")
        .h(px(25.0))
        .px(px(6.0))
        .rounded(px(5.0))
        .border_1()
        .border_color(row_border)
        .flex()
        .items_center()
        .gap(px(7.0))
        .cursor_pointer()
        .bg(row_bg)
        .hover(move |s| s.bg(row_hover))
        .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
            let target_id = this.ensure_daily_queue_scheme(this.daily_queue_today, cx);
            if let Some(popup) = this.event_popup.as_ref() {
                if popup.scheme_id != target_id {
                    this.move_popup_item_to_scheme(target_id, cx);
                } else if let Some(popup) = this.event_popup.as_mut() {
                    popup.scheme_menu_open = false;
                }
            }
            cx.stop_propagation();
            cx.notify();
        }))
        .child(
            div()
                .w(px(9.0))
                .h(px(9.0))
                .rounded(px(2.0))
                .flex_shrink_0()
                .bg(token_rgba(daily_queue_marker_color(t.is_dark))),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_size(px(12.0))
                .line_height(px(16.0))
                .font_family(FONT_UI)
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(token_hsla(t.text_primary))
                .child(DAILY_QUEUE_TITLE),
        )
        .into_any_element()
}

fn event_scheme_picker_folder_row(
    name: &str,
    id: FolderId,
    depth: usize,
    t: Theme,
) -> gpui::AnyElement {
    div()
        .id(SharedString::from(format!("popup-scheme-folder-{}", id)))
        .h(px(23.0))
        .pl(px(6.0 + depth as f32 * 11.0))
        .pr(px(6.0))
        .flex()
        .items_center()
        .gap(px(6.0))
        .text_size(px(11.0))
        .line_height(px(15.0))
        .font_family(FONT_UI)
        .text_color(token_hsla(t.text_dim))
        .child(
            div()
                .w(px(12.0))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    Icon::empty()
                        .path(SCHEME_PICKER_FOLDER_ICON)
                        .with_size(px(FOLDER_ICON_SIZE))
                        .text_color(token_hsla(t.text_dim))
                        .into_any_element(),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(name.to_string()),
        )
        .into_any_element()
}

fn event_scheme_picker_scheme_row(
    target_id: SchemeId,
    name: String,
    color_index: u8,
    depth: usize,
    selected: bool,
    t: Theme,
    cx: &mut Context<KnotQApp>,
) -> gpui::AnyElement {
    let item_color = calendar_item_color(false, color_index, t.is_dark);
    let row_bg = scheme_picker_row_bg(item_color, selected, t);
    let row_hover = scheme_picker_row_hover(item_color, selected, t);
    let row_border = scheme_picker_row_border(item_color, selected, t);

    div()
        .id(SharedString::from(format!(
            "popup-scheme-pick-{}",
            target_id
        )))
        .h(px(25.0))
        .pl(px(6.0 + depth as f32 * 11.0))
        .pr(px(6.0))
        .rounded(px(5.0))
        .border_1()
        .border_color(row_border)
        .flex()
        .items_center()
        .gap(px(7.0))
        .cursor_pointer()
        .bg(row_bg)
        .hover(move |s| s.bg(row_hover))
        .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
            if let Some(popup) = this.event_popup.as_ref() {
                if popup.scheme_id != target_id {
                    this.move_popup_item_to_scheme(target_id, cx);
                } else if let Some(popup) = this.event_popup.as_mut() {
                    popup.scheme_menu_open = false;
                }
            }
            cx.stop_propagation();
            cx.notify();
        }))
        .child(
            div()
                .w(px(9.0))
                .h(px(9.0))
                .rounded(px(2.0))
                .flex_shrink_0()
                .bg(item_color),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_size(px(12.0))
                .line_height(px(16.0))
                .font_family(FONT_UI)
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(token_hsla(t.text_primary))
                .child(name),
        )
        .into_any_element()
}

fn scheme_picker_row_bg(color: gpui::Hsla, selected: bool, t: Theme) -> gpui::Hsla {
    with_alpha(
        color,
        if selected {
            if t.is_dark {
                0.2
            } else {
                0.14
            }
        } else {
            0.0
        },
    )
}

fn scheme_picker_row_hover(color: gpui::Hsla, selected: bool, t: Theme) -> gpui::Hsla {
    with_alpha(
        color,
        if selected {
            if t.is_dark {
                0.26
            } else {
                0.18
            }
        } else if t.is_dark {
            0.12
        } else {
            0.09
        },
    )
}

fn scheme_picker_row_border(color: gpui::Hsla, selected: bool, t: Theme) -> gpui::Hsla {
    with_alpha(
        color,
        if selected {
            if t.is_dark {
                0.44
            } else {
                0.34
            }
        } else {
            0.0
        },
    )
}

fn with_alpha(mut color: gpui::Hsla, alpha: f32) -> gpui::Hsla {
    color.a = alpha;
    color
}
