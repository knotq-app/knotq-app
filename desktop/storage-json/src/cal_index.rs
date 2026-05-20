use chrono::{Datelike, NaiveDate};
use knotq_model::{Item, ItemKind, ItemMarker, Recurrence, Scheme};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct SchemeCalendarIndex {
    #[serde(default, skip_serializing_if = "crate::files::is_false")]
    pub(crate) has_calendar_items: bool,
    #[serde(default, skip_serializing_if = "crate::files::is_false")]
    pub(crate) has_unfinished_items: bool,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub(crate) months_with_calendar_items: BTreeSet<YearMonth>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub(crate) months_with_unfinished_items: BTreeSet<YearMonth>,
    #[serde(default, skip_serializing_if = "crate::files::is_false")]
    pub(crate) load_for_calendar_queries: bool,
    #[serde(default, skip_serializing_if = "crate::files::is_false")]
    pub(crate) load_for_unfinished_queries: bool,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub(crate) struct YearMonth {
    pub(crate) year: i32,
    pub(crate) month: u32,
}

impl SchemeCalendarIndex {
    pub(crate) fn from_scheme(scheme: &Scheme) -> Self {
        let mut index = Self::default();
        for item in &scheme.items {
            let item_has_unfinished = item_has_unfinished_state(item);
            if item_has_unfinished {
                index.has_unfinished_items = true;
            }

            let kind = item.kind();
            let is_calendar_item = matches!(
                kind,
                ItemKind::Assignment | ItemKind::Event | ItemKind::Reminder
            );
            if !is_calendar_item {
                continue;
            }

            index.has_calendar_items = true;
            for month in calendar_months_for_item(item, kind) {
                index.months_with_calendar_items.insert(month);
                if item_has_unfinished {
                    index.months_with_unfinished_items.insert(month);
                }
            }

            if item.repeats.is_some() {
                index.load_for_calendar_queries = true;
                if item_has_unfinished {
                    index.load_for_unfinished_queries = true;
                }
            }
        }
        index
    }
}

pub(crate) fn daily_queue_calendar_index_matches_range(
    index: &SchemeCalendarIndex,
    start: Option<NaiveDate>,
    end: Option<NaiveDate>,
) -> bool {
    let (Some(start), Some(end)) = (start, end) else {
        return false;
    };
    if index.has_unfinished_items {
        return true;
    }
    if index.load_for_calendar_queries || index.load_for_unfinished_queries {
        return true;
    }

    let first = YearMonth::from_date(start.min(end));
    let last = YearMonth::from_date(start.max(end));
    months_intersect_range(&index.months_with_calendar_items, first, last)
        || months_intersect_range(&index.months_with_unfinished_items, first, last)
}

fn calendar_months_for_item(item: &Item, kind: ItemKind) -> Vec<YearMonth> {
    match kind {
        ItemKind::Event => match (item.start, item.end) {
            (Some(start), Some(end)) => months_between_dates(start.date_naive(), end.date_naive()),
            (Some(start), None) => vec![YearMonth::from_date(start.date_naive())],
            (None, Some(end)) => vec![YearMonth::from_date(end.date_naive())],
            (None, None) => Vec::new(),
        },
        ItemKind::Reminder => item
            .start
            .map(|start| vec![YearMonth::from_date(start.date_naive())])
            .unwrap_or_default(),
        ItemKind::Assignment => item
            .end
            .map(|end| vec![YearMonth::from_date(end.date_naive())])
            .unwrap_or_default(),
        ItemKind::Procedure => Vec::new(),
    }
}

fn months_between_dates(start: NaiveDate, end: NaiveDate) -> Vec<YearMonth> {
    let first = YearMonth::from_date(start.min(end));
    let last = YearMonth::from_date(start.max(end));
    let mut months = Vec::new();
    let mut current = first;
    loop {
        months.push(current);
        if current == last {
            break;
        }
        current = current.next();
    }
    months
}

fn months_intersect_range(months: &BTreeSet<YearMonth>, first: YearMonth, last: YearMonth) -> bool {
    months.iter().any(|month| *month >= first && *month <= last)
}

fn item_has_unfinished_state(item: &Item) -> bool {
    if item.marker != ItemMarker::Checkbox {
        return false;
    }
    if item.state.is_empty() {
        return true;
    }
    if recurrence_has_open_future(item.repeats.as_ref()) {
        return true;
    }
    item.state.iter().any(|entry| !entry.state.is_done())
}

fn recurrence_has_open_future(recurrence: Option<&Recurrence>) -> bool {
    recurrence.is_some()
}

impl YearMonth {
    fn from_date(date: NaiveDate) -> Self {
        Self {
            year: date.year(),
            month: date.month(),
        }
    }

    fn next(self) -> Self {
        if self.month == 12 {
            Self {
                year: self.year + 1,
                month: 1,
            }
        } else {
            Self {
                year: self.year,
                month: self.month + 1,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use knotq_model::{Item, ItemMarker, Scheme};

    use super::*;

    #[test]
    fn calendar_index_records_months_spanned_by_events() {
        let mut scheme = Scheme::new("Travel", 0);
        let mut item = Item::new("retreat");
        item.marker = ItemMarker::Checkbox;
        item.start = Some(Utc.with_ymd_and_hms(2026, 1, 31, 23, 0, 0).unwrap());
        item.end = Some(Utc.with_ymd_and_hms(2026, 3, 1, 0, 0, 0).unwrap());
        scheme.items.push(item);

        let index = SchemeCalendarIndex::from_scheme(&scheme);

        assert!(index.months_with_calendar_items.contains(&YearMonth {
            year: 2026,
            month: 1
        }));
        assert!(index.months_with_calendar_items.contains(&YearMonth {
            year: 2026,
            month: 2
        }));
        assert!(index.months_with_calendar_items.contains(&YearMonth {
            year: 2026,
            month: 3
        }));
    }
}
