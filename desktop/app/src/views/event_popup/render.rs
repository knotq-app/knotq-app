use super::*;

impl KnotQApp {
    pub fn render_event_popup(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let popup = self.event_popup.clone()?;
        let scheme = self.workspace.scheme(popup.scheme_id)?.clone();
        let item = scheme.item(popup.item_id)?.clone();
        let t = self.theme();
        let scheme_id = popup.scheme_id;
        let item_id = popup.item_id;
        let editable = !scheme.is_read_only();

        let mut draft_item = item.clone();
        draft_item.start = popup.draft_start;
        draft_item.end = popup.draft_end;
        draft_item.repeats = popup.draft_repeats.clone();
        let mut base_repeat_item = item.clone();
        base_repeat_item.repeats = popup.draft_repeats.clone();

        let kind = draft_item.kind();
        let is_done = popup.draft_done;
        let can_toggle_done = editable;
        let has_repeating_occurrence = item.repeats.is_some() && !popup.occurrence.is_single();
        let date_presence_changed = has_repeating_occurrence
            && ((popup.start_dirty && item.start.is_some() != popup.draft_start.is_some())
                || (popup.end_dirty && item.end.is_some() != popup.draft_end.is_some()));
        let title = item_title(&item.text);
        let title_input = popup.title_input.clone();
        let start = popup.draft_start;
        let end = popup.draft_end;
        let is_daily = self.workspace.is_daily_queue_scheme(scheme_id);
        let accent = if is_daily {
            token_hsla(daily_queue_marker_color(t.is_dark))
        } else {
            calendar_item_color(false, scheme.color_index, t.is_dark)
        };
        let notification_offset = popup
            .draft_notification_offset_secs
            .unwrap_or_else(|| default_notification_offset(kind, self.notification_defaults));
        let repeats_summary = repeat_summary(popup.draft_repeats.as_ref(), self.time_format);
        let scheme_label = self.scheme_display_name(&scheme);
        let notification_menu_open = popup.notification_menu_open && editable;
        let repeat_menu_open = popup.repeat_menu_open && editable;
        let scope_action = popup.scope_action;
        let scope_dialog_open = scope_action.is_some();
        let until_picker_open = popup.until_picker_open;
        let until_calendar_anchor_y = popup.until_calendar_anchor_y;
        let until_display_month = until_display_month_for_popup(&popup, start, end);
        let viewport_width = px(f32::from(window.viewport_size().width));
        let viewport_height = px(f32::from(window.viewport_size().height));
        let desired_card_top = popup.anchor.y + px(8.0);
        let card_top = popover_top_biased_below(
            desired_card_top,
            px(EVENT_POPUP_ESTIMATED_HEIGHT),
            viewport_height,
        );
        let card_left = clamped_popup_left(popup.anchor.x, px(EVENT_POPUP_WIDTH), viewport_width);
        let date_popover_width = if self.time_format == TimeFormat::TwelveHour {
            DATE_POPOVER_WIDTH_12H
        } else {
            DATE_POPOVER_WIDTH_24H
        };
        let date_popover_x = clamped_popup_left(
            card_left + px(EVENT_POPUP_WIDTH - date_popover_width),
            px(date_popover_width),
            viewport_width,
        );
        let date_popover_y_offset = px(DATE_POPOVER_Y_OFFSET);
        let notification_menu_left = clamped_popup_left(
            card_left + px(NOTIFICATION_MENU_LEFT_OFFSET),
            px(NOTIFICATION_MENU_WIDTH),
            viewport_width,
        );
        let notification_menu_top = popover_top_biased_below(
            card_top + px(67.0),
            px(NOTIFICATION_MENU_HEIGHT),
            viewport_height,
        );
        let repeat_menu_left = clamped_popup_left(
            card_left + px(REPEAT_MENU_LEFT_OFFSET),
            px(REPEAT_MENU_WIDTH),
            viewport_width,
        );
        let repeat_menu_top = popover_top_biased_below(
            card_top + px(REPEAT_MENU_TOP_OFFSET),
            px(REPEAT_MENU_HEIGHT),
            viewport_height,
        );
        let scheme_menu_left = clamped_popup_left(
            card_left + px(14.0 + 16.0 + EVENT_POPUP_HEADER_GAP),
            px(SCHEME_PICKER_WIDTH),
            viewport_width,
        );
        let scheme_menu_top =
            popover_top_biased_below(card_top + px(55.0), px(260.0), viewport_height);
        let scheme_menu = (popup.scheme_menu_open && editable).then(|| {
            self.render_event_scheme_picker(scheme_id, scheme_menu_left, scheme_menu_top, t, cx)
        });

        let scrim = div()
            .id("event-scrim")
            .absolute()
            .inset_0()
            .bg(token_rgba(if scope_dialog_open {
                t.overlay_scrim
            } else {
                0x00000000
            }))
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                if scope_dialog_open {
                    this.cancel_event_scope_dialog(cx);
                } else {
                    this.close_event_popup(cx);
                    this.focus_app_root(window);
                }
                cx.notify();
            }));

        let card = div()
            .id("event-popup-card")
            .absolute()
            .left(card_left)
            .top(card_top)
            .w(px(EVENT_POPUP_WIDTH))
            .bg(token_hsla(t.bg_modal))
            .border_1()
            .border_color(token_rgba(t.border_overlay))
            .rounded(px(8.0))
            .shadow_lg()
            .occlude()
            .overflow_hidden()
            .flex()
            .flex_col()
            .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                let mut changed = false;
                if let Some(popup) = this.event_popup.as_mut() {
                    changed = popup.notification_menu_open
                        || popup.repeat_menu_open
                        || popup.until_picker_open
                        || popup.scheme_menu_open;
                    popup.close_all_menus();
                }
                cx.stop_propagation();
                if changed {
                    cx.notify();
                }
            }))
            .child(
                div()
                    .w_full()
                    .h(px(EVENT_POPUP_HEADER_LIP_H))
                    .bg(token_rgba(if t.is_dark { 0xffffff0d } else { 0x00000008 })),
            )
            .child(
                div()
                    .w_full()
                    .px(px(14.0))
                    .pt(px(8.0))
                    .pb(px(8.0))
                    .bg(token_rgba(if t.is_dark { 0xffffff0d } else { 0x00000008 }))
                    .child(
                        div().flex().flex_col().gap(px(2.0)).child(
                            div()
                                .flex()
                                .items_start()
                                .gap(px(EVENT_POPUP_HEADER_GAP))
                                .child(
                                    div()
                                        .id("popup-done-checkbox")
                                        .flex_shrink_0()
                                        .mt(px(3.0))
                                        .opacity(if can_toggle_done { 1.0 } else { 0.35 })
                                        .child(task_checkbox(is_done, t))
                                        .when(can_toggle_done, |s| s.cursor_pointer())
                                        .on_click(cx.listener(
                                            move |this, _: &ClickEvent, _w, cx| {
                                                if !can_toggle_done {
                                                    return;
                                                }
                                                if let Some(popup) = this.event_popup.as_mut() {
                                                    popup.close_all_menus();
                                                    popup.draft_done = !popup.draft_done;
                                                    popup.done_dirty = true;
                                                }
                                                cx.stop_propagation();
                                                cx.notify();
                                            },
                                        )),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .flex()
                                        .flex_col()
                                        .gap(px(2.0))
                                        .child(event_title_input(title_input, title, t))
                                        .child(event_scheme_chip(
                                            scheme_label,
                                            accent,
                                            scheme_id,
                                            item_id,
                                            editable,
                                            t,
                                            cx,
                                        )),
                                )
                                .when(editable, |row| {
                                    row.child(div().flex_shrink_0().mt(px(3.0)).child(
                                        delete_event_icon_button(has_repeating_occurrence, t, cx),
                                    ))
                                })
                                .when(!editable, |row| {
                                    row.child(
                                        div()
                                            .flex_shrink_0()
                                            .mt(px(1.0))
                                            .child(read_only_event_badge(t)),
                                    )
                                }),
                        ),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .h(px(EVENT_POPUP_HEADER_SEPARATOR_H))
                    .bg(accent),
            )
            .child(
                div()
                    .w_full()
                    .px(px(14.0))
                    .pt(px(8.0))
                    .pb(px(14.0))
                    .relative()
                    .flex()
                    .flex_col()
                    .gap(px(5.0))
                    .child(editable_detail_row(
                        "popup-notification-row",
                        "Notification",
                        format_lead_time(notification_offset),
                        t,
                        editable,
                        cx.listener(move |this, _: &ClickEvent, _window, cx| {
                            if let Some(popup) = this.event_popup.as_mut() {
                                popup.notification_menu_open = !popup.notification_menu_open;
                                popup.repeat_menu_open = false;
                                popup.until_picker_open = false;
                                popup.scheme_menu_open = false;
                            }
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    ))
                    .child(editable_detail_row(
                        "popup-start-row",
                        "Start",
                        format_optional_datetime(self.time_format, start, true),
                        t,
                        editable,
                        cx.listener(move |this, event: &ClickEvent, window, cx| {
                            if let Some(popup) = this.event_popup.as_mut() {
                                popup.close_all_menus();
                            }
                            let anchor =
                                point(date_popover_x, event.position().y + date_popover_y_offset);
                            this.open_date_popover(
                                scheme_id,
                                item_id,
                                DateKind::Start,
                                anchor,
                                window,
                                cx,
                            );
                            cx.stop_propagation();
                        }),
                    ))
                    .child(editable_detail_row(
                        "popup-end-row",
                        "End",
                        format_optional_datetime(self.time_format, end, true),
                        t,
                        editable,
                        cx.listener(move |this, event: &ClickEvent, window, cx| {
                            if let Some(popup) = this.event_popup.as_mut() {
                                popup.close_all_menus();
                            }
                            let anchor =
                                point(date_popover_x, event.position().y + date_popover_y_offset);
                            this.open_date_popover(
                                scheme_id,
                                item_id,
                                DateKind::End,
                                anchor,
                                window,
                                cx,
                            );
                            cx.stop_propagation();
                        }),
                    ))
                    .child(editable_detail_row(
                        "popup-repeat-row",
                        "Repeat",
                        repeats_summary,
                        t,
                        editable,
                        cx.listener(move |this, _: &ClickEvent, _window, cx| {
                            if let Some(popup) = this.event_popup.as_mut() {
                                let toggle = !popup.repeat_menu_open;
                                popup.close_all_menus();
                                popup.repeat_menu_open = toggle;
                            }
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    ))
                    .when(editable, |section| {
                        section.when_some(popup.draft_repeats.as_ref(), |section, repeat| {
                            section.child(repeat_details_inline_editor(
                                repeat,
                                &base_repeat_item,
                                scheme_id,
                                item_id,
                                t,
                                cx,
                            ))
                        })
                    }),
            );

        let layer = event_popup_layer(
            card.into_any_element(),
            scrim.into_any_element(),
            scope_dialog_open,
            scope_action,
            date_presence_changed,
            &popup,
            &item,
            notification_menu_open,
            notification_offset,
            notification_menu_left,
            notification_menu_top,
            repeat_menu_open,
            repeat_menu_left,
            repeat_menu_top,
            scheme_menu,
            until_picker_open,
            until_display_month,
            until_calendar_anchor_y,
            card_left,
            viewport_width,
            viewport_height,
            scheme_id,
            item_id,
            t,
            cx,
        );

        Some(
            deferred(layer)
                .with_priority(EVENT_POPUP_PRIORITY)
                .into_any_element(),
        )
    }

    fn render_event_scheme_picker(
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

fn event_title_input(
    input: Option<Entity<SingleLineEditor>>,
    fallback: String,
    t: Theme,
) -> gpui::AnyElement {
    let base = div()
        .id("popup-title-input")
        .w_full()
        .min_w_0()
        .h(px(22.0))
        .overflow_hidden()
        .text_size(px(15.0))
        .line_height(px(19.0))
        .font_family(FONT_UI)
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(token_hsla(t.text_primary));
    if let Some(input) = input {
        base.child(input).into_any_element()
    } else {
        base.whitespace_nowrap()
            .text_ellipsis()
            .child(fallback)
            .into_any_element()
    }
}

fn event_scheme_chip(
    label: String,
    _accent: gpui::Hsla,
    scheme_id: SchemeId,
    item_id: ItemId,
    editable: bool,
    t: Theme,
    cx: &mut Context<KnotQApp>,
) -> gpui::AnyElement {
    let underline = token_hsla(t.text_primary);

    div()
        .id("popup-scheme-row")
        .max_w_full()
        .flex()
        .items_center()
        .gap(px(2.0))
        .font_family(FONT_UI)
        .child(
            div()
                .id("popup-scheme-label")
                .min_w_0()
                .max_w(px(EVENT_POPUP_WIDTH - 56.0))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_size(px(12.0))
                .line_height(px(15.0))
                .text_color(token_hsla(t.text_soft))
                .cursor_pointer()
                .border_b_1()
                .border_color(token_rgba(0x00000000))
                .hover(move |s| {
                    s.text_color(token_hsla(t.text_primary))
                        .border_color(underline)
                })
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.open_scheme(scheme_id, Some(item_id));
                    this.focus_current_editor(window, cx);
                    this.close_event_popup(cx);
                    cx.stop_propagation();
                    cx.notify();
                }))
                .child(label),
        )
        .when(editable, |row| {
            row.child(
                div()
                    .id("popup-scheme-picker-toggle")
                    .w(px(19.0))
                    .h(px(18.0))
                    .rounded(px(4.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(12.0))
                    .font_family(FONT_UI)
                    .text_color(token_hsla(t.text_soft))
                    .bg(token_rgba(0x00000000))
                    .cursor_pointer()
                    .hover({
                        let hover = t.button_hover;
                        move |s| {
                            s.bg(token_rgba(hover))
                                .text_color(token_hsla(t.text_primary))
                        }
                    })
                    .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                        if let Some(popup) = this.event_popup.as_mut() {
                            let toggle = !popup.scheme_menu_open;
                            popup.close_all_menus();
                            popup.scheme_menu_open = toggle;
                        }
                        cx.stop_propagation();
                        cx.notify();
                    }))
                    .child(
                        Icon::empty()
                            .path(SCHEME_PICKER_MOVE_ICON)
                            .with_size(px(14.0))
                            .text_color(token_hsla(t.text_soft))
                            .into_any_element(),
                    ),
            )
        })
        .into_any_element()
}

fn read_only_event_badge(t: Theme) -> gpui::AnyElement {
    div()
        .id("popup-read-only-badge")
        .h(px(19.0))
        .px(px(7.0))
        .rounded(px(5.0))
        .border_1()
        .border_color(token_rgba(t.border_soft))
        .bg(token_rgba(t.button_bg))
        .flex()
        .items_center()
        .justify_center()
        .font_family(FONT_UI)
        .text_size(px(10.0))
        .line_height(px(12.0))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(token_hsla(t.text_soft))
        .child("Read only")
        .into_any_element()
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
                        .with_size(px(SCHEME_PICKER_FOLDER_ICON_SIZE))
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
