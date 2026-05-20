use super::*;

impl KnotQApp {
    pub(super) fn date_for_target(&self, target: DateTarget) -> Option<chrono::DateTime<Utc>> {
        if let Some(date) = self.event_popup_date(target) {
            return date;
        }
        self.workspace
            .scheme(target.scheme_id)
            .and_then(|scheme| scheme.item(target.item_id))
            .and_then(|item| match target.kind {
                DateKind::Start => item.start,
                DateKind::End => item.end,
                DateKind::Available => item.available,
            })
    }

    fn event_popup_date(&self, target: DateTarget) -> Option<Option<chrono::DateTime<Utc>>> {
        let popup = self.event_popup.as_ref()?;
        if popup.scheme_id != target.scheme_id || popup.item_id != target.item_id {
            return None;
        }
        match target.kind {
            DateKind::Start => Some(popup.draft_start),
            DateKind::End => Some(popup.draft_end),
            DateKind::Available => None,
        }
    }

    fn date_popover_item_for_target(&self, target: DateTarget) -> Option<Item> {
        let mut item = self
            .workspace
            .scheme(target.scheme_id)
            .and_then(|scheme| scheme.item(target.item_id))
            .cloned()?;
        if let Some(popup) = self.event_popup.as_ref() {
            if popup.scheme_id == target.scheme_id && popup.item_id == target.item_id {
                item.start = popup.draft_start;
                item.end = popup.draft_end;
                item.repeats = popup.draft_repeats.clone();
            }
        }
        Some(item)
    }

    pub(super) fn set_event_popup_date(
        &mut self,
        target: DateTarget,
        date: Option<chrono::DateTime<Utc>>,
    ) -> bool {
        if self.workspace.is_scheme_read_only(target.scheme_id) {
            return false;
        }
        let Some(popup) = self.event_popup.as_mut() else {
            return false;
        };
        if popup.scheme_id != target.scheme_id || popup.item_id != target.item_id {
            return false;
        }
        match target.kind {
            DateKind::Start => {
                popup.draft_start = date;
                popup.start_dirty = true;
            }
            DateKind::End => {
                popup.draft_end = date;
                popup.end_dirty = true;
            }
            DateKind::Available => return false,
        }
        true
    }

    pub(super) fn set_target_date(
        &mut self,
        target: DateTarget,
        date: chrono::DateTime<Utc>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.workspace.is_scheme_read_only(target.scheme_id) {
            return;
        }
        if !self.set_event_popup_date(target, Some(date)) {
            self.apply(
                Command::SetItemDate {
                    scheme: target.scheme_id,
                    item: target.item_id,
                    kind: target.kind,
                    date: Some(date),
                },
                cx,
            );
        }
        self.sync_date_popover_inputs(date, window, cx);
        cx.notify();
    }

    fn sync_date_popover_inputs(
        &mut self,
        date: chrono::DateTime<Utc>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let time_format = self.time_format;
        let Some(popup) = self.date_popover.as_mut() else {
            return;
        };
        let local = date.with_timezone(&Local);
        popup.hour_is_pm = local.hour() >= 12;
        popup.year_input.update(cx, |input, cx| {
            set_input_value_if_changed(input, format!("{:04}", local.year()), window, cx);
        });
        popup.month_input.update(cx, |input, cx| {
            set_input_value_if_changed(input, format!("{:02}", local.month()), window, cx);
        });
        popup.day_input.update(cx, |input, cx| {
            set_input_value_if_changed(input, format!("{:02}", local.day()), window, cx);
        });
        popup.hour_input.update(cx, |input, cx| {
            set_input_value_if_changed(
                input,
                popover_hour_value(time_format, local.hour()),
                window,
                cx,
            );
        });
        popup.minute_input.update(cx, |input, cx| {
            set_input_value_if_changed(input, format!("{:02}", local.minute()), window, cx);
        });
    }

    pub fn open_date_popover(
        &mut self,
        scheme_id: SchemeId,
        item_id: ItemId,
        kind: DateKind,
        anchor: gpui::Point<gpui::Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.workspace.is_scheme_read_only(scheme_id) {
            return;
        }
        self.close_repeat_popover();
        let target = DateTarget {
            scheme_id,
            item_id,
            kind,
        };
        let mut initial_date = None;
        if let Some(item) = self.date_popover_item_for_target(target) {
            let current = match kind {
                DateKind::Start => item.start,
                DateKind::End => item.end,
                DateKind::Available => item.available,
            };
            initial_date = current;
            if initial_date.is_none() {
                let date = default_datetime(kind, &item);
                if !self.set_event_popup_date(target, Some(date)) {
                    self.apply(
                        Command::SetItemDate {
                            scheme: scheme_id,
                            item: item_id,
                            kind,
                            date: Some(date),
                        },
                        cx,
                    );
                }
                initial_date = Some(date);
            }
        }
        let current_local = initial_date
            .unwrap_or_else(rounded_local_now_utc)
            .with_timezone(&Local);
        let hour_is_pm = current_local.hour() >= 12;
        let time_format = self.time_format;
        let year_input = cx.new(|cx| {
            DateComponentField::new(
                "yyyy",
                format!("{:04}", current_local.year()),
                4,
                window,
                cx,
            )
        });
        let month_input = cx.new(|cx| {
            DateComponentField::new("mm", format!("{:02}", current_local.month()), 2, window, cx)
        });
        let day_input = cx.new(|cx| {
            DateComponentField::new("dd", format!("{:02}", current_local.day()), 2, window, cx)
        });
        let hour_input = cx.new(|cx| {
            DateComponentField::new(
                "hh",
                popover_hour_value(time_format, current_local.hour()),
                2,
                window,
                cx,
            )
        });
        let minute_input = cx.new(|cx| {
            DateComponentField::new(
                "mm",
                format!("{:02}", current_local.minute()),
                2,
                window,
                cx,
            )
        });
        let year_sub = cx.subscribe_in(&year_input, window, Self::on_date_popover_input_event);
        let month_sub = cx.subscribe_in(&month_input, window, Self::on_date_popover_input_event);
        let day_sub = cx.subscribe_in(&day_input, window, Self::on_date_popover_input_event);
        let hour_sub = cx.subscribe_in(&hour_input, window, Self::on_date_popover_input_event);
        let minute_sub = cx.subscribe_in(&minute_input, window, Self::on_date_popover_input_event);
        day_input.update(cx, |input, cx| input.focus(window, cx));
        self.date_popover = Some(DatePickerPopover {
            scheme_id,
            item_id,
            kind,
            anchor,
            hour_is_pm,
            year_input,
            month_input,
            day_input,
            hour_input,
            minute_input,
            _year_subscription: year_sub,
            _month_subscription: month_sub,
            _day_subscription: day_sub,
            _hour_subscription: hour_sub,
            _minute_subscription: minute_sub,
        });
    }

    pub fn close_date_popover(&mut self) {
        self.date_popover = None;
    }

    fn on_date_popover_input_event(
        &mut self,
        input: &Entity<DateComponentField>,
        event: &DateComponentEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            DateComponentEvent::Change => {
                self.apply_date_popover_inputs(cx);
            }
            DateComponentEvent::Filled => {
                self.apply_date_popover_inputs(cx);
                self.focus_next_date_popover_input(input, window, cx);
            }
            DateComponentEvent::PressEnter => {
                self.apply_date_popover_inputs(cx);
                if !self.focus_next_date_popover_input(input, window, cx) {
                    self.focus_current_editor(window, cx);
                }
            }
            DateComponentEvent::TabForward => {
                self.apply_date_popover_inputs(cx);
                self.focus_wrapped_date_popover_input(input, false, window, cx);
            }
            DateComponentEvent::TabBackward => {
                self.apply_date_popover_inputs(cx);
                self.focus_wrapped_date_popover_input(input, true, window, cx);
            }
            DateComponentEvent::Cancel => {
                self.focus_current_editor(window, cx);
            }
            DateComponentEvent::Undo => {
                self.undo(cx);
                self.sync_date_popover_after_history(window, cx);
            }
            DateComponentEvent::Redo => {
                self.redo(cx);
                self.sync_date_popover_after_history(window, cx);
            }
            DateComponentEvent::Focus | DateComponentEvent::Blur => {
                cx.notify();
            }
        }
    }

    fn focus_wrapped_date_popover_input(
        &self,
        input: &Entity<DateComponentField>,
        backward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(popup) = self.date_popover.as_ref() else {
            return false;
        };

        let next = if backward {
            if input == &popup.year_input {
                popup.minute_input.clone()
            } else if input == &popup.month_input {
                popup.year_input.clone()
            } else if input == &popup.day_input {
                popup.month_input.clone()
            } else if input == &popup.hour_input {
                popup.day_input.clone()
            } else {
                popup.hour_input.clone()
            }
        } else if input == &popup.year_input {
            popup.month_input.clone()
        } else if input == &popup.month_input {
            popup.day_input.clone()
        } else if input == &popup.day_input {
            popup.hour_input.clone()
        } else if input == &popup.hour_input {
            popup.minute_input.clone()
        } else {
            popup.year_input.clone()
        };

        next.update(cx, |input, cx| input.focus(window, cx));
        true
    }

    fn focus_next_date_popover_input(
        &self,
        input: &Entity<DateComponentField>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(popup) = self.date_popover.as_ref() else {
            return false;
        };

        let next = if input == &popup.year_input {
            Some(popup.month_input.clone())
        } else if input == &popup.month_input {
            Some(popup.day_input.clone())
        } else if input == &popup.day_input {
            Some(popup.hour_input.clone())
        } else if input == &popup.hour_input {
            Some(popup.minute_input.clone())
        } else {
            None
        };

        if let Some(next) = next {
            next.update(cx, |input, cx| input.focus(window, cx));
            true
        } else {
            false
        }
    }

    pub(super) fn apply_date_popover_inputs(&mut self, cx: &mut Context<Self>) {
        let Some(popup) = self.date_popover.as_ref() else {
            return;
        };
        let target = (popup.scheme_id, popup.item_id, popup.kind);
        let hour_is_pm = popup.hour_is_pm;
        let year = popup.year_input.read(cx).value().to_string();
        let month = popup.month_input.read(cx).value().to_string();
        let day = popup.day_input.read(cx).value().to_string();
        let hour = popup.hour_input.read(cx).value().to_string();
        let minute = popup.minute_input.read(cx).value().to_string();
        let Some(date) = parse_popover_datetime(
            self.time_format,
            &year,
            &month,
            &day,
            &hour,
            &minute,
            hour_is_pm,
        ) else {
            return;
        };
        let target = DateTarget {
            scheme_id: target.0,
            item_id: target.1,
            kind: target.2,
        };
        if !self.set_event_popup_date(target, Some(date)) {
            self.apply(
                Command::SetItemDate {
                    scheme: target.scheme_id,
                    item: target.item_id,
                    kind: target.kind,
                    date: Some(date),
                },
                cx,
            );
        }
    }

    fn sync_date_popover_after_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(popup) = self.date_popover.as_ref() else {
            return;
        };
        let target = DateTarget {
            scheme_id: popup.scheme_id,
            item_id: popup.item_id,
            kind: popup.kind,
        };
        if let Some(date) = self.date_for_target(target) {
            self.sync_date_popover_inputs(date, window, cx);
        } else {
            self.close_date_popover();
        }
        cx.notify();
    }
}
