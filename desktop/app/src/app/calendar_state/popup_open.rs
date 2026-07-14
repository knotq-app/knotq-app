use chrono::{DateTime, NaiveDate, Utc};
use gpui::{AppContext, Context, Pixels, Point, Window};
use knotq_commands::Command;
use knotq_date_util::snapped_calendar_datetime;
use knotq_model::{Item, ItemId, ItemMarker, OccurrenceId, SchemeId};
use knotq_ui::single_line_editor::{SingleLineEditor, SingleLineEditorEvent};

use crate::app::{EventPopup, KnotQApp};

impl KnotQApp {
    pub(crate) fn open_event_popup(
        &mut self,
        scheme_id: SchemeId,
        item_id: ItemId,
        occurrence: OccurrenceId,
        occurrence_index: usize,
        start: Option<DateTime<Utc>>,
        end: Option<DateTime<Utc>>,
        anchor: Point<Pixels>,
        select_title: bool,
        created_from_calendar: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.event_popup.is_some() {
            self.close_event_popup(cx);
            if self.event_popup.is_some() {
                return;
            }
        } else {
            self.close_date_popover();
        }
        self.close_repeat_popover();
        let Some(scheme) = self.workspace.scheme(scheme_id) else {
            return;
        };
        let read_only = scheme.is_read_only();
        let Some(item) = scheme.item(item_id).cloned() else {
            return;
        };
        let occurrence_state = item.state_for_occurrence(&occurrence);
        let title_input = if read_only {
            self.event_popup_title_subscription = None;
            None
        } else {
            let input = cx.new(|cx| SingleLineEditor::new("Title", item.text(), window, cx));
            let title_subscription =
                cx.subscribe_in(&input, window, Self::on_event_popup_title_input_event);
            if select_title {
                input.update(cx, |input, cx| input.focus_and_select_all(window, cx));
            }
            self.event_popup_title_subscription = Some(title_subscription);
            Some(input)
        };
        let draft_start = if occurrence.is_single() {
            item.start
        } else {
            start.or(item.start)
        };
        let draft_end = if occurrence.is_single() {
            item.end
        } else {
            end.or(item.end)
        };
        let mut popup = EventPopup::new(
            scheme_id,
            item_id,
            &item,
            occurrence,
            &occurrence_state,
            draft_start,
            draft_end,
            anchor,
            occurrence_index,
        );
        popup.title_input = title_input;
        popup.created_from_calendar = created_from_calendar;
        self.event_popup = Some(popup);
        cx.notify();
    }

    fn on_event_popup_title_input_event(
        &mut self,
        input: &gpui::Entity<SingleLineEditor>,
        event: &SingleLineEditorEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let matching_input = self.event_popup.as_ref().is_some_and(|popup| {
            popup
                .title_input
                .as_ref()
                .is_some_and(|title| title == input)
        });
        if !matching_input {
            return;
        }
        let value = input.read(cx).value().to_string();

        match event {
            SingleLineEditorEvent::Change | SingleLineEditorEvent::Submit => {
                self.update_event_popup_title_draft(value, cx);
                if matches!(event, SingleLineEditorEvent::Submit) {
                    self.close_event_popup(cx);
                    self.focus_app_root(window);
                }
            }
            SingleLineEditorEvent::Blur => {
                self.update_event_popup_title_draft(value, cx);
            }
            SingleLineEditorEvent::Cancel => {
                self.cancel_event_popup_without_commit(cx);
                self.focus_app_root(window);
                cx.notify();
            }
            SingleLineEditorEvent::Focus => {}
        }
    }

    fn update_event_popup_title_draft(&mut self, text: String, cx: &mut Context<Self>) {
        let read_only = self
            .event_popup
            .as_ref()
            .is_some_and(|popup| self.workspace.is_scheme_read_only(popup.scheme_id));
        if read_only {
            return;
        }
        if let Some(popup) = self.event_popup.as_mut() {
            if popup.draft_title != text {
                popup.draft_title = text;
                popup.title_dirty = true;
                cx.notify();
            }
        }
    }
    /// Create a new calendar item from a click or drag on the week view.
    /// - Click → reminder (start date = clicked time)
    /// - Shift+click → assignment (end date = clicked time)
    /// - Drag → event (start + end from drag range)
    ///
    /// The item is added to today's daily queue scheme by default, then the
    /// event popup is opened so the user can customize it.
    /// `start_hour`/`end_hour` are hour fractions (0.0–24.0).
    pub(crate) fn create_calendar_item_from_drag(
        &mut self,
        date: NaiveDate,
        start_hour: f32,
        end_hour: f32,
        shift: bool,
        anchor: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let is_drag = (end_hour - start_hour).abs() > 0.125; // ~7.5 minutes

        let hour_to_datetime = |hour: f32| snapped_calendar_datetime(date, hour);

        let scheme_id = self.ensure_daily_queue_scheme(self.daily_queue_today, cx);

        let mut item = Item::new("");
        item.marker = ItemMarker::Checkbox;

        if is_drag {
            // Event: start + end from drag range.
            let (lo, hi) = if start_hour < end_hour {
                (start_hour, end_hour)
            } else {
                (end_hour, start_hour)
            };
            item.start = Some(hour_to_datetime(lo));
            item.end = Some(hour_to_datetime(hi));
        } else if shift {
            // Assignment: end only.
            item.end = Some(hour_to_datetime(start_hour));
        } else {
            // Reminder: start only.
            item.start = Some(hour_to_datetime(start_hour));
        }

        let item_id = item.id;
        let position = self
            .workspace
            .scheme(scheme_id)
            .map(|s| s.items.len())
            .unwrap_or(0);
        self.apply(
            Command::InsertItem {
                scheme: scheme_id,
                position,
                item,
            },
            cx,
        );

        self.open_event_popup(
            scheme_id,
            item_id,
            OccurrenceId::Single,
            0,
            self.workspace
                .scheme(scheme_id)
                .and_then(|s| s.item(item_id))
                .and_then(|i| i.start),
            self.workspace
                .scheme(scheme_id)
                .and_then(|s| s.item(item_id))
                .and_then(|i| i.end),
            anchor,
            true,
            true,
            window,
            cx,
        );
    }

    /// Move the item from the current popup's scheme to a different scheme.
    pub(crate) fn move_popup_item_to_scheme(
        &mut self,
        target_scheme_id: SchemeId,
        cx: &mut Context<Self>,
    ) {
        let Some((source_scheme_id, item_id, created_from_calendar)) = self
            .event_popup
            .as_ref()
            .map(|popup| (popup.scheme_id, popup.item_id, popup.created_from_calendar))
        else {
            return;
        };
        if self.workspace.is_scheme_read_only(source_scheme_id)
            || self.workspace.is_scheme_read_only(target_scheme_id)
        {
            return;
        }
        if source_scheme_id == target_scheme_id {
            if let Some(popup) = self.event_popup.as_mut() {
                popup.scheme_menu_open = false;
            }
            cx.notify();
            return;
        }

        let Some(item) = self
            .workspace
            .scheme(source_scheme_id)
            .and_then(|s| s.item(item_id))
            .cloned()
        else {
            return;
        };

        let Some(position) = self
            .workspace
            .scheme(target_scheme_id)
            .map(|s| s.items.len())
        else {
            return;
        };

        // Keep the popup alive while reconciliation runs after the move batch.
        if let Some(popup) = self.event_popup.as_mut() {
            popup.scheme_id = target_scheme_id;
            popup.scheme_menu_open = false;
        }

        let command = Command::Batch(vec![
            Command::DeleteItem {
                scheme: source_scheme_id,
                item: item_id,
            },
            Command::InsertItem {
                scheme: target_scheme_id,
                position,
                item,
            },
        ]);
        let applied = if created_from_calendar {
            self.apply_without_pushing_undo(command, cx)
        } else {
            self.apply(command, cx)
        };
        if applied.is_some() {
            if created_from_calendar {
                self.retarget_pending_creation_undo(item_id, target_scheme_id);
            }
        } else if let Some(popup) = self.event_popup.as_mut() {
            popup.scheme_id = source_scheme_id;
        }
    }
}
