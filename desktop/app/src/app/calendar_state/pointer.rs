use chrono::Utc;
use gpui::Context;
use knotq_model::{ItemId, OccurrenceId, SchemeId};

use crate::app::{CalendarOccurrenceKey, KnotQApp};

impl KnotQApp {
    pub(crate) fn clear_calendar_pointer_state(&mut self, cx: &mut Context<Self>) -> bool {
        let had_state =
            self.cal_drag.is_some() || self.cal_move.is_some() || self.cal_resize.is_some();
        if had_state {
            self.cal_drag = None;
            self.cal_move = None;
            self.cal_resize = None;
            cx.notify();
        }
        had_state
    }

    pub(crate) fn retains_completed_calendar_item(
        &self,
        scheme_id: SchemeId,
        item_id: ItemId,
        occurrence: &OccurrenceId,
    ) -> bool {
        self.retained_completed().is_retained(
            &CalendarOccurrenceKey {
                scheme_id,
                item_id,
                occurrence: occurrence.clone(),
            },
            Utc::now(),
        )
    }

    pub(crate) fn sync_retained_completed_calendar_items(
        &mut self,
        keys: &[CalendarOccurrenceKey],
    ) {
        for key in keys.iter().cloned() {
            if self.calendar_occurrence_is_done(&key) {
                self.retained_completed_mut().insert(key, Utc::now());
            } else {
                self.retained_completed_mut().remove(&key);
            }
        }
    }

    fn calendar_occurrence_is_done(&self, key: &CalendarOccurrenceKey) -> bool {
        self.workspace
            .scheme(key.scheme_id)
            .and_then(|scheme| scheme.item(key.item_id))
            .map(|item| item.state_for_occurrence(&key.occurrence))
            .unwrap_or_default()
            .is_done()
    }
}
