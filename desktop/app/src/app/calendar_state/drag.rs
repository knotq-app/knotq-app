use chrono::Utc;
use gpui::{Context, Window};
use knotq_commands::{reset_after_trigger_notification_to_default_command, Command};
use knotq_date_util::snapped_calendar_datetime;

use crate::app::{EventPopup, EventPopupInit, EventScopeAction, KnotQApp, OpenEventPopupArgs};

impl KnotQApp {
    /// Resolve the end of a drag-to-move gesture. A negligible drag (no day
    /// change and no snapped time change) is treated as a plain click and opens
    /// the event popup — mirroring the old per-block `on_click`, which the
    /// gesture overlay now intercepts. Anything larger commits the move.
    pub(crate) fn finish_calendar_move(
        &mut self,
        mv: crate::app::CalendarMoveState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if mv.is_negligible() {
            self.open_event_popup(
                OpenEventPopupArgs {
                    scheme_id: mv.scheme_id,
                    item_id: mv.item_id,
                    occurrence: mv.occurrence,
                    occurrence_index: mv.occurrence_index,
                    start: mv.occurrence_start,
                    end: mv.occurrence_end,
                    anchor: mv.anchor,
                    select_title: false,
                    created_from_calendar: false,
                },
                window,
                cx,
            );
            return;
        }
        self.commit_calendar_move(mv, cx);
    }

    /// Commit a drag-to-move operation, applying time offset to the item's dates.
    pub(crate) fn commit_calendar_move(
        &mut self,
        mv: crate::app::CalendarMoveState,
        cx: &mut Context<Self>,
    ) {
        if self.workspace.is_scheme_read_only(mv.scheme_id) {
            return;
        }
        if mv.is_negligible() {
            // Negligible move — treat as a click (the title on_click will handle popup).
            return;
        }

        let Some(item) = self
            .workspace
            .scheme(mv.scheme_id)
            .and_then(|s| s.item(mv.item_id))
            .cloned()
        else {
            return;
        };

        // Same source of truth as the drag ghost — what was previewed is exactly
        // what we commit.
        let (draft_start, draft_end) = mv.draft_dates();
        let start_dirty = draft_start != mv.occurrence_start;
        let end_dirty = draft_end != mv.occurrence_end;
        if !start_dirty && !end_dirty {
            return;
        }

        if item.repeats.is_some() && !mv.occurrence.is_single() {
            self.close_date_popover();
            self.close_repeat_popover();
            let occurrence_state = item.state_for_occurrence(&mv.occurrence);
            let mut popup = EventPopup::new(EventPopupInit {
                scheme_id: mv.scheme_id,
                item_id: mv.item_id,
                item: &item,
                occurrence: mv.occurrence.clone(),
                occurrence_state: &occurrence_state,
                draft_start,
                draft_end,
                anchor: mv.anchor,
                occurrence_index: mv.occurrence_index,
            });
            popup.start_dirty = start_dirty;
            popup.end_dirty = end_dirty;
            if let Some(Command::SetOccurrenceNotificationOffset { offset_secs, .. }) =
                reset_after_trigger_notification_to_default_command(
                    &item,
                    mv.scheme_id,
                    mv.item_id,
                    mv.occurrence.clone(),
                    draft_start,
                    draft_end,
                    Utc::now(),
                )
            {
                popup.draft_notification_offset_secs = offset_secs;
                popup.notification_dirty = true;
            }
            popup.scope_action = Some(EventScopeAction::ApplyChanges);
            popup.scope_dialog_only = true;
            self.event_popup = Some(popup);
            cx.notify();
            return;
        }

        let mut commands = Vec::new();
        if start_dirty {
            commands.push(Command::SetItemDate {
                scheme: mv.scheme_id,
                item: mv.item_id,
                kind: knotq_commands::DateKind::Start,
                date: draft_start,
            });
        }
        if end_dirty {
            commands.push(Command::SetItemDate {
                scheme: mv.scheme_id,
                item: mv.item_id,
                kind: knotq_commands::DateKind::End,
                date: draft_end,
            });
        }
        if let Some(command) = reset_after_trigger_notification_to_default_command(
            &item,
            mv.scheme_id,
            mv.item_id,
            mv.occurrence,
            draft_start,
            draft_end,
            Utc::now(),
        ) {
            commands.push(command);
        }

        if let Some(cmd) = Command::from_vec(commands) {
            self.apply(cmd, cx);
        }
    }

    /// Commit a bottom-edge resize operation, normalizing inverted endpoints.
    pub(crate) fn commit_calendar_resize(
        &mut self,
        resize: crate::app::CalendarResizeState,
        cx: &mut Context<Self>,
    ) {
        if self.workspace.is_scheme_read_only(resize.scheme_id) {
            return;
        }
        let proposed_end = snapped_calendar_datetime(resize.date, resize.current_hour);
        let (draft_start, draft_end) = if proposed_end < resize.occurrence_start {
            (proposed_end, resize.occurrence_start)
        } else {
            (resize.occurrence_start, proposed_end)
        };
        let draft_start = Some(draft_start);
        let draft_end = Some(draft_end);
        let start_dirty = draft_start != Some(resize.occurrence_start);
        let end_dirty = draft_end != Some(resize.occurrence_end);
        if !start_dirty && !end_dirty {
            return;
        }

        let Some(item) = self
            .workspace
            .scheme(resize.scheme_id)
            .and_then(|s| s.item(resize.item_id))
            .cloned()
        else {
            return;
        };

        if item.repeats.is_some() && !resize.occurrence.is_single() {
            self.close_date_popover();
            self.close_repeat_popover();
            let occurrence_state = item.state_for_occurrence(&resize.occurrence);
            let mut popup = EventPopup::new(EventPopupInit {
                scheme_id: resize.scheme_id,
                item_id: resize.item_id,
                item: &item,
                occurrence: resize.occurrence,
                occurrence_state: &occurrence_state,
                draft_start,
                draft_end,
                anchor: resize.anchor,
                occurrence_index: resize.occurrence_index,
            });
            popup.start_dirty = start_dirty;
            popup.end_dirty = end_dirty;
            popup.scope_action = Some(EventScopeAction::ApplyChanges);
            popup.scope_dialog_only = true;
            self.event_popup = Some(popup);
            cx.notify();
            return;
        }

        let mut commands = Vec::new();
        if start_dirty {
            commands.push(Command::SetItemDate {
                scheme: resize.scheme_id,
                item: resize.item_id,
                kind: knotq_commands::DateKind::Start,
                date: draft_start,
            });
        }
        if end_dirty {
            commands.push(Command::SetItemDate {
                scheme: resize.scheme_id,
                item: resize.item_id,
                kind: knotq_commands::DateKind::End,
                date: draft_end,
            });
        }

        if let Some(cmd) = Command::from_vec(commands) {
            self.apply(cmd, cx);
        }
    }
}
