use super::super::*;

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
        // Completion is local-only state, so it can always be toggled — even on a
        // read-only (imported) event whose content is otherwise uneditable.
        let can_toggle_done = true;
        let has_repeating_occurrence = item.repeats.is_some() && !popup.occurrence.is_single();
        let date_presence_changed = has_repeating_occurrence
            && ((popup.start_dirty && item.start.is_some() != popup.draft_start.is_some())
                || (popup.end_dirty && item.end.is_some() != popup.draft_end.is_some()));
        let title = item_title(&item.text());
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
        let notification_trigger = notification_trigger_at(kind, start, end);
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
                        // Explicit pixel width (card width minus the 14px side
                        // padding) rather than `w_full`: in GPUI a percentage
                        // width on a flex *container* gets overridden by
                        // shrink-to-fit, collapsing the row so the flex_1 title
                        // column gets no room (the title then ellipsizes to "...").
                        // A fixed length keeps the row full width so the title
                        // shows, even next to the "Read only" badge.
                        div()
                            .w(px(EVENT_POPUP_WIDTH - 28.0))
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(
                            div()
                                .w(px(EVENT_POPUP_WIDTH - 28.0))
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
                        knotq_l10n::t("event.field.notification"),
                        format_lead_time(
                            self.time_format,
                            notification_offset,
                            notification_trigger,
                        ),
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
                        knotq_l10n::t("event.field.start"),
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
                        knotq_l10n::t("event.field.end"),
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
                        knotq_l10n::t("event.field.repeat"),
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
}

fn event_title_input(
    input: Option<Entity<SingleLineEditor>>,
    fallback: String,
    t: Theme,
) -> gpui::AnyElement {
    let base = div()
        .id("popup-title-input")
        .min_w_0()
        .h(px(22.0))
        .overflow_hidden()
        .text_size(px(15.0))
        .line_height(px(19.0))
        .font_family(FONT_UI)
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(token_hsla(t.text_primary));
    if let Some(input) = input {
        // Editable: the text editor fills the column.
        base.w_full().child(input).into_any_element()
    } else {
        // Read-only: wrap the title in a flex row so it's a content-sized
        // (capped) item, exactly like the scheme chip below. A bare text div
        // placed directly in the flex-column gets stretched to the column's
        // collapsed width and ellipsizes down to just "...".
        div()
            .flex()
            .max_w_full()
            .child(
                base.max_w(px(EVENT_POPUP_WIDTH - 126.0))
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(fallback),
            )
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

    // Clicking the scheme name opens the picker so the event can be moved to a
    // different scheme (editable schemes only). The trailing arrow "goes to
    // definition" — it jumps to this item inside its scheme editor — and stays
    // available even for read-only schemes since navigation is always safe.
    let name = div()
        .id("popup-scheme-label")
        .min_w_0()
        .max_w(px(EVENT_POPUP_WIDTH - 56.0))
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .text_size(px(12.0))
        .line_height(px(15.0))
        .text_color(token_hsla(t.text_soft))
        .border_b_1()
        .border_color(token_rgba(0x00000000))
        .child(label);
    let name = if editable {
        name.cursor_pointer()
            .hover(move |s| {
                s.text_color(token_hsla(t.text_primary))
                    .border_color(underline)
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
    } else {
        name
    };

    div()
        .id("popup-scheme-row")
        .max_w_full()
        .flex()
        .items_center()
        .gap(px(2.0))
        .font_family(FONT_UI)
        .child(name)
        .child(
            div()
                .id("popup-scheme-goto")
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
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.open_scheme(scheme_id, Some(item_id));
                    this.focus_current_editor(window, cx);
                    this.close_event_popup(cx);
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
        .child(knotq_l10n::t("event.badge.read_only"))
        .into_any_element()
}
