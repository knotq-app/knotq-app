use super::*;

impl KnotQApp {
    pub fn render_repeat_popover(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let popup = self.repeat_popover.as_ref()?;

        let target = RepeatTarget {
            scheme_id: popup.scheme_id,
            item_id: popup.item_id,
        };

        let item = self.item_for_repeat_target(target);
        let existing = self.repeat_for_target(target);
        let state = if let Some(item) = item {
            existing
                .as_ref()
                .and_then(|repeat| repeat_state_from_recurrence(repeat, item))
                .unwrap_or_default()
                .normalized(item)
        } else {
            RepeatState::default()
        };
        let has_simple_repeat = existing
            .as_ref()
            .and_then(|repeat| item.and_then(|item| repeat_state_from_recurrence(repeat, item)))
            .is_some();
        let complex_repeat = existing.is_some() && !has_simple_repeat;
        let is_recurring = has_simple_repeat || complex_repeat;
        let scope = popup.scope;
        let until_open = popup.until_open;
        let current_until = match &state.end {
            RepeatEnd::Until(until) => Some(until.with_timezone(&Local).date_naive()),
            _ => None,
        };
        let until_display_month = {
            let base = popup
                .until_display_month
                .unwrap_or_else(|| current_until.unwrap_or_else(|| Local::now().date_naive()));
            NaiveDate::from_ymd_opt(base.year(), base.month(), 1).unwrap_or(base)
        };
        let occurrence_index = popup.occurrence_index;
        let selected_days = state.weekdays.clone();
        let type_menu_open = popup.type_menu_open;
        let active_mode = if has_simple_repeat {
            Some(state.mode)
        } else {
            None
        };
        let repeat_type_value = if complex_repeat {
            knotq_l10n::t("repeat.type.custom").to_string()
        } else if has_simple_repeat {
            state.mode.label().to_string()
        } else {
            knotq_l10n::t("repeat.type.none").to_string()
        };

        let t = self.theme();
        let viewport_width = px(f32::from(window.viewport_size().width));
        let viewport_height = px(f32::from(window.viewport_size().height));
        let desired_left = if popup.anchor.x == px(0.0) {
            px(420.0)
        } else {
            popup.anchor.x
        };
        let left = clamped_popover_left(desired_left, px(REPEAT_POPOVER_WIDTH), viewport_width);
        let desired_top = if popup.anchor.y == px(0.0) {
            px(132.0)
        } else {
            popup.anchor.y
        };
        let top = popover_top_biased_below(
            desired_top,
            px(repeat_popover_estimated_height(
                is_recurring && occurrence_index.is_some(),
                has_simple_repeat,
                state.mode == RepeatMode::Weekly,
            )),
            viewport_height,
        );
        let scope_row_count = if is_recurring && occurrence_index.is_some() {
            2.0
        } else {
            0.0
        };
        let repeat_row_top = top + px(scope_row_count * 25.0);
        let type_menu_top = popover_top_biased_below(
            repeat_row_top + px(25.0),
            px(repeat_type_menu_height(complex_repeat)),
            viewport_height,
        );
        let weekly_row_count = if has_simple_repeat && state.mode == RepeatMode::Weekly {
            1.0
        } else {
            0.0
        };
        let end_row_top = top + px((scope_row_count + 1.0 + weekly_row_count) * 25.0);
        let until_calendar_left = clamped_popover_left(
            left + px(REPEAT_POPOVER_WIDTH - UNTIL_CALENDAR_WIDTH),
            px(UNTIL_CALENDAR_WIDTH),
            viewport_width,
        );
        let until_calendar_top = popover_top_biased_below(
            end_row_top + px(25.0),
            px(UNTIL_CALENDAR_HEIGHT),
            viewport_height,
        );

        let scrim = div()
            .id("repeat-popover-scrim")
            .absolute()
            .inset_0()
            .bg(token_rgba(0x00000001))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                this.close_repeat_popover();
                this.focus_app_root(window);
                cx.stop_propagation();
                cx.notify();
            }));

        let card = div()
            .id("repeat-popover-card")
            .absolute()
            .left(left)
            .top(top)
            .w(px(REPEAT_POPOVER_WIDTH))
            .bg(token_hsla(t.bg_modal))
            .border_1()
            .border_color(token_rgba(t.border_overlay))
            .rounded(px(6.0))
            .shadow_lg()
            .overflow_hidden()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_action(cx.listener(|this, _: &InputEscape, window, cx| {
                this.focus_current_editor(window, cx);
                cx.stop_propagation();
            }))
            .on_click(|_: &ClickEvent, _w, cx| cx.stop_propagation())
            // Scope section: shown when editing a specific occurrence of a recurring event
            .when(is_recurring && occurrence_index.is_some(), |card| {
                card.child(rp_row(
                    "rp-scope-this",
                    knotq_l10n::t("repeat.scope.this_event_only"),
                    scope == RepeatScope::ThisEvent,
                    t,
                    cx.listener(move |this, _: &ClickEvent, _w, cx| {
                        if let Some(p) = this.repeat_popover.as_mut() {
                            p.scope = RepeatScope::ThisEvent;
                        }
                        cx.stop_propagation();
                        cx.notify();
                    }),
                ))
                .child(rp_row(
                    "rp-scope-future",
                    knotq_l10n::t("repeat.scope.this_and_future"),
                    scope == RepeatScope::AllFuture,
                    t,
                    cx.listener(move |this, _: &ClickEvent, _w, cx| {
                        if let Some(p) = this.repeat_popover.as_mut() {
                            p.scope = RepeatScope::AllFuture;
                        }
                        cx.stop_propagation();
                        cx.notify();
                    }),
                ))
            })
            .child(rp_selector_row(
                "rp-type-select",
                knotq_l10n::t("repeat.field.repeat"),
                repeat_type_value,
                type_menu_open,
                t,
                cx.listener(move |this, _: &ClickEvent, _w, cx| {
                    if let Some(popup) = this.repeat_popover.as_mut() {
                        popup.type_menu_open = !popup.type_menu_open;
                        popup.end_menu_open = false;
                        popup.until_open = false;
                    }
                    cx.stop_propagation();
                    cx.notify();
                }),
            ))
            // Weekday selector row: shown when weekly mode is active
            .when(
                has_simple_repeat && state.mode == RepeatMode::Weekly,
                |card| {
                    card.child(
                        div()
                            .h(px(25.0))
                            .px(px(14.0))
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .font_family(crate::theme_gpui::FONT_UI)
                            .text_size(px(11.0))
                            .child(
                                div()
                                    .w(px(112.0))
                                    .flex_shrink_0()
                                    .text_color(token_hsla(t.text_dim))
                                    .child(""),
                            )
                            .child(div().min_w_0().flex().items_center().gap(px(2.0)).children(
                                repeat_weekday_labels().iter().map(|day| {
                                    let active = selected_days.contains(day);
                                    rp_weekday_chip(*day, active, target, t, cx)
                                }),
                            )),
                    )
                },
            )
            // Ends section: shown when a simple repeat mode is active
            .when(has_simple_repeat, |card| {
                let event_datetime = item.and_then(|item| item.start.or(item.end));
                card.child(rp_repeat_end_row(state.end.clone(), event_datetime, t, cx))
            });

        let layer = div()
            .absolute()
            .inset_0()
            .child(scrim)
            .child(card)
            .when(type_menu_open, |layer| {
                layer.child(rp_repeat_type_menu(
                    existing.is_some(),
                    complex_repeat,
                    active_mode,
                    target,
                    left,
                    type_menu_top,
                    t,
                    cx,
                ))
            })
            .when(until_open && has_simple_repeat, |layer| {
                layer.child(rp_until_calendar(
                    until_display_month,
                    current_until,
                    target,
                    until_calendar_left,
                    until_calendar_top,
                    t,
                    cx,
                ))
            });

        Some(
            deferred(layer)
                .with_priority(REPEAT_POPOVER_PRIORITY)
                .into_any_element(),
        )
    }
}
