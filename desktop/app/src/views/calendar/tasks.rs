use super::*;

impl KnotQApp {
    pub(super) fn collect_calendar_tasks(
        &self,
        start_utc: chrono::DateTime<Utc>,
        end_utc: chrono::DateTime<Utc>,
    ) -> Vec<CalendarTask> {
        let mut all_tasks = Vec::new();
        for scheme in self.workspace.iter_schemes() {
            let is_daily = self.workspace.is_daily_queue_scheme(scheme.id);
            let is_read_only = scheme.is_read_only();
            for item in &scheme.items {
                for occ in item.occurrences(start_utc, end_utc) {
                    all_tasks.push(CalendarTask {
                        scheme_id: scheme.id,
                        item_id: item.id,
                        occurrence: occ.id,
                        occurrence_index: occ.occurrence_index,
                        color_index: scheme.color_index,
                        is_daily,
                        is_read_only,
                        text: item.text(),
                        start: occ.start.map(|d| d.with_timezone(&Local)),
                        end: occ.end.map(|d| d.with_timezone(&Local)),
                        kind: occ.kind,
                        is_done: occ.state.is_done(),
                    });
                }
            }
        }
        all_tasks
    }
}
