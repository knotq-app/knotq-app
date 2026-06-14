use super::*;

impl KnotQApp {
    pub fn ensure_daily_queue_scheme(
        &mut self,
        date: NaiveDate,
        cx: &mut Context<Self>,
    ) -> SchemeId {
        self.ensure_daily_queue_scheme_internal(date, true, cx)
    }

    pub(crate) fn ensure_daily_queue_scheme_quiet(
        &mut self,
        date: NaiveDate,
        cx: &mut Context<Self>,
    ) -> SchemeId {
        self.ensure_daily_queue_scheme_internal(date, false, cx)
    }

    fn ensure_daily_queue_scheme_internal(
        &mut self,
        date: NaiveDate,
        should_notify: bool,
        cx: &mut Context<Self>,
    ) -> SchemeId {
        if let Some(existing) = self.workspace.daily_queue_scheme_id(date) {
            if self.workspace.scheme(existing).is_some() {
                self.ensure_daily_queue_blank_placeholder(existing, should_notify, cx);
                return existing;
            }
            match load_daily_queue_scheme(&workspace_path(), date) {
                Ok(Some(mut scheme)) if scheme.id == existing => {
                    ensure_plain_blank_daily_row(&mut scheme);
                    self.workspace.schemes.insert(existing, scheme);
                    self.state.mark_direct_workspace_dirty();
                    if should_notify {
                        cx.notify();
                    }
                    return existing;
                }
                Ok(Some(scheme)) => {
                    eprintln!(
                        "daily queue {} loaded with unexpected id {}, expected {}",
                        date, scheme.id, existing
                    );
                    return existing;
                }
                Ok(None) => {}
                Err(err) => {
                    eprintln!("daily queue load failed for {date}: {err:#}");
                    return existing;
                }
            }
            self.workspace.daily_queue.remove(&date);
            self.state.mark_direct_workspace_dirty();
        }

        let id = knotq_model::daily_queue_scheme_id(date);
        let mut scheme = Scheme::new(daily_queue_scheme_name(date), DAILY_QUEUE_COLOR_INDEX);
        scheme.id = id;
        ensure_plain_blank_daily_row(&mut scheme);
        self.workspace.daily_queue.insert(date, id);
        self.workspace.schemes.insert(id, scheme);
        self.workspace
            .scheme_sync
            .insert(id, knotq_model::daily_queue_sync_metadata(date));
        self.state.mark_scheme_dirty(id);
        self.service_bus.signal_save();
        if should_notify {
            cx.notify();
        }
        id
    }

    fn ensure_daily_queue_blank_placeholder(
        &mut self,
        scheme_id: SchemeId,
        should_notify: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(scheme) = self.workspace.schemes.get_mut(&scheme_id) else {
            return;
        };
        if !ensure_plain_blank_daily_row(scheme) {
            return;
        }
        self.state.mark_scheme_dirty(scheme_id);
        self.service_bus.signal_save();
        if should_notify {
            cx.notify();
        }
    }

    pub(crate) fn ensure_daily_queue_calendar_range_loaded(
        &mut self,
        start: NaiveDate,
        end: NaiveDate,
        cx: &mut Context<Self>,
    ) {
        let months = calendar_month_keys_between(start, end);
        if months
            .iter()
            .all(|month| self.daily_queue_loaded_calendar_months.contains(month))
        {
            return;
        }

        match load_daily_queue_schemes_for_calendar_range(&workspace_path(), start, end) {
            Ok(schemes) => {
                let mut changed = false;
                for (date, scheme) in schemes {
                    let Some(expected_id) = self.workspace.daily_queue_scheme_id(date) else {
                        continue;
                    };
                    if expected_id != scheme.id {
                        eprintln!(
                            "daily queue {} loaded with unexpected id {}, expected {}",
                            date, scheme.id, expected_id
                        );
                        continue;
                    }
                    if self.workspace.schemes.contains_key(&scheme.id) {
                        continue;
                    }
                    self.workspace.schemes.insert(scheme.id, scheme);
                    self.state.mark_direct_workspace_dirty();
                    changed = true;
                }
                self.daily_queue_loaded_calendar_months.extend(months);
                if changed {
                    cx.notify();
                }
            }
            Err(err) => {
                eprintln!(
                    "daily queue calendar range load failed for {} through {}: {err:#}",
                    start.min(end),
                    start.max(end)
                );
            }
        }
    }

    pub fn ensure_daily_queue_window(&mut self, cx: &mut Context<Self>) -> Vec<NaiveDate> {
        let today = self.daily_queue_today;
        let start = self.daily_queue_loaded_start.min(today);
        let end = today;
        let mut dates = self
            .workspace
            .daily_queue
            .range(start..=end)
            .map(|(date, _)| *date)
            .collect::<Vec<_>>();
        if !dates.contains(&today) {
            dates.push(today);
            dates.sort();
        }
        for date in &dates {
            self.ensure_daily_queue_scheme_quiet(*date, cx);
        }
        dates
    }

    pub(crate) fn expand_daily_queue_older(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(earliest) = self.workspace.daily_queue.keys().next().copied() else {
            return false;
        };
        if self.daily_queue_loaded_start <= earliest {
            return false;
        }
        self.daily_queue_preserved_bottom_distance =
            Some(self.daily_queue_scroll_distance_from_bottom());
        let requested = self.daily_queue_loaded_start - Duration::days(DAILY_QUEUE_PAGE_DAYS);
        self.daily_queue_loaded_start = requested.max(earliest);
        cx.notify();
        true
    }

    fn daily_queue_scroll_distance_from_bottom(&self) -> Pixels {
        let max_y = self.daily_queue_scroll_handle.max_offset().height;
        (max_y + self.daily_queue_scroll_handle.offset().y).clamp(Pixels::ZERO, max_y)
    }

    pub fn carryover_daily_queue(&mut self, cx: &mut Context<Self>) {
        let today = self.daily_queue_today;
        let yesterday = today - Duration::days(1);
        let Some(previous_id) = self.workspace.daily_queue_scheme_id(yesterday) else {
            return;
        };
        let today_id = self.ensure_daily_queue_scheme(today, cx);
        let command = {
            let Some(previous) = self.workspace.scheme(previous_id) else {
                return;
            };
            let Some(today_scheme) = self.workspace.scheme(today_id) else {
                return;
            };
            daily_queue_carryover_command(previous_id, previous, today_id, today_scheme)
        };
        let Some(command) = command else {
            return;
        };
        if self.apply(command, cx).is_none() {
            return;
        }
        self.close_date_popover();
        self.close_repeat_popover();
        self.selection.view = View::DailyQueue;
        self.selection.scheme_id = Some(today_id);
        self.selection.focused_item_id = self
            .workspace
            .scheme(today_id)
            .and_then(|scheme| scheme.items.last())
            .map(|item| item.id);
        self.daily_queue_scroll_initialized = false;
        cx.notify();
    }

    pub(crate) fn sync_daily_queue_day_boundary(&mut self, cx: &mut Context<Self>) {
        let today = Local::now().date_naive();
        self.sync_daily_queue_day_boundary_to(today, cx);
    }

    pub(crate) fn sync_daily_queue_day_boundary_to(
        &mut self,
        today: NaiveDate,
        cx: &mut Context<Self>,
    ) {
        if today == self.daily_queue_today {
            return;
        }

        self.daily_queue_today = today;
        self.daily_queue_loaded_start = daily_queue_default_window_start(today);
        self.daily_queue_preserved_bottom_distance = None;
        self.daily_queue_scroll_initialized = false;
        self.daily_queue_visible_dates.clear();

        if self.selection.view == View::DailyQueue {
            self.ensure_daily_queue_window(cx);
        }
        cx.notify();
    }
}

fn ensure_plain_blank_daily_row(scheme: &mut Scheme) -> bool {
    if !scheme.items.is_empty() {
        return false;
    }
    scheme.items.push(Item::new(""));
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use knotq_model::ItemMarker;

    #[test]
    fn empty_daily_scheme_gets_plain_blank_placeholder_row() {
        let mut scheme = Scheme::new("Daily 2026-06-05", DAILY_QUEUE_COLOR_INDEX);

        assert!(ensure_plain_blank_daily_row(&mut scheme));
        assert_eq!(scheme.items.len(), 1);
        assert_eq!(scheme.items[0].text, "");
        assert_eq!(scheme.items[0].marker, ItemMarker::Blank);
        assert_eq!(scheme.items[0].indent, 0);
        assert!(!ensure_plain_blank_daily_row(&mut scheme));
        assert_eq!(scheme.items.len(), 1);
    }
}
