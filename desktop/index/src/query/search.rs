use chrono::Local;
use knotq_date_util::format_time;
use knotq_model::{Item, ItemId, ItemKind, SchemeId, TimeFormat, Workspace};

use crate::IndexedWorkspace;

#[derive(Clone, Copy, Debug)]
pub struct SearchOptions<'a> {
    pub daily_queue_title: &'a str,
    pub daily_queue_marker_color: u32,
}

pub struct SearchQuery<'a> {
    indexed: &'a IndexedWorkspace,
    time_format: TimeFormat,
    options: SearchOptions<'a>,
}

impl<'a> SearchQuery<'a> {
    pub fn new(
        indexed: &'a IndexedWorkspace,
        time_format: TimeFormat,
        options: SearchOptions<'a>,
    ) -> Self {
        Self {
            indexed,
            time_format,
            options,
        }
    }

    pub fn run(&self, text: &str) -> Vec<SearchHit> {
        search_hits(
            &self.indexed.workspace,
            self.time_format,
            text,
            self.options,
        )
    }
}

#[derive(Clone, Debug)]
pub struct SearchHit {
    pub target: SearchTarget,
    pub scheme_name: String,
    pub color_index: Option<u8>,
    pub color_override: Option<u32>,
    pub title: String,
    pub detail: String,
    pub status: SearchHitStatus,
}

#[derive(Clone, Debug)]
pub enum SearchTarget {
    Calendar,
    DailyQueue {
        scheme_id: Option<SchemeId>,
        item_id: Option<ItemId>,
    },
    Scheme {
        scheme_id: SchemeId,
        item_id: Option<ItemId>,
    },
}

#[derive(Clone, Copy, Debug, Default)]
pub enum SearchHitStatus {
    #[default]
    None,
    Date {
        dt: chrono::DateTime<chrono::Utc>,
    },
    Event {
        start: chrono::DateTime<chrono::Utc>,
        end: Option<chrono::DateTime<chrono::Utc>>,
    },
    DailyQueue,
}

pub fn search_hits(
    workspace: &Workspace,
    time_format: TimeFormat,
    query: &str,
    options: SearchOptions<'_>,
) -> Vec<SearchHit> {
    let query = query.trim();
    let mut hits = Vec::new();

    push_navigation_hits(&mut hits, query, options);
    push_scheme_hits(&mut hits, workspace, time_format, query);
    push_daily_queue_hits(&mut hits, workspace, time_format, query, options);

    hits.truncate(60);
    hits
}

fn push_navigation_hits(hits: &mut Vec<SearchHit>, query: &str, options: SearchOptions<'_>) {
    if matches_query("Calendar", query) {
        hits.push(SearchHit {
            target: SearchTarget::Calendar,
            scheme_name: "Navigation".to_string(),
            color_index: None,
            color_override: None,
            title: "Calendar".to_string(),
            detail: "view".to_string(),
            status: SearchHitStatus::None,
        });
    }
    if matches_query(options.daily_queue_title, query) {
        hits.push(SearchHit {
            target: SearchTarget::DailyQueue {
                scheme_id: None,
                item_id: None,
            },
            scheme_name: "Navigation".to_string(),
            color_index: None,
            color_override: Some(options.daily_queue_marker_color),
            title: options.daily_queue_title.to_string(),
            detail: "view".to_string(),
            status: SearchHitStatus::None,
        });
    }
}

fn push_scheme_hits(
    hits: &mut Vec<SearchHit>,
    workspace: &Workspace,
    time_format: TimeFormat,
    query: &str,
) {
    for scheme in workspace
        .iter_schemes()
        .filter(|scheme| !workspace.is_daily_queue_scheme(scheme.id))
    {
        for item in &scheme.items {
            if !item_has_search_title(item) {
                continue;
            }
            let (detail, status) = item_detail(item, time_format);
            if !matches_search_hit(&item.text, &scheme.name, &detail, query) {
                continue;
            }
            hits.push(SearchHit {
                target: SearchTarget::Scheme {
                    scheme_id: scheme.id,
                    item_id: Some(item.id),
                },
                scheme_name: scheme.name.clone(),
                color_index: Some(scheme.color_index),
                color_override: None,
                title: item.text.clone(),
                detail,
                status,
            });
        }
    }
}

fn push_daily_queue_hits(
    hits: &mut Vec<SearchHit>,
    workspace: &Workspace,
    time_format: TimeFormat,
    query: &str,
    options: SearchOptions<'_>,
) {
    for (date, scheme) in workspace.iter_daily_queue_schemes() {
        let day_label = format!("{}", date.format("%Y %B %-d"));
        for item in &scheme.items {
            if !item_has_search_title(item) {
                continue;
            }
            let (detail, _) = item_detail(item, time_format);
            let matches = matches_query(&item.text, query)
                || matches_query(options.daily_queue_title, query)
                || matches_query(&day_label, query)
                || matches_query(&detail, query);
            if !matches {
                continue;
            }
            hits.push(SearchHit {
                target: SearchTarget::DailyQueue {
                    scheme_id: Some(scheme.id),
                    item_id: Some(item.id),
                },
                scheme_name: options.daily_queue_title.to_string(),
                color_index: None,
                color_override: Some(options.daily_queue_marker_color),
                title: item.text.clone(),
                detail,
                status: SearchHitStatus::DailyQueue,
            });
        }
    }
}

fn matches_search_hit(title: &str, scheme: &str, detail: &str, query: &str) -> bool {
    matches_query(title, query) || matches_query(scheme, query) || matches_query(detail, query)
}

fn matches_query(text: &str, query: &str) -> bool {
    query.trim().is_empty() || subsequence_match_indices(text, query).is_some()
}

fn subsequence_match_indices(text: &str, query: &str) -> Option<Vec<usize>> {
    let query: Vec<char> = query
        .trim()
        .chars()
        .flat_map(|ch| ch.to_lowercase())
        .collect();
    if query.is_empty() {
        return Some(Vec::new());
    }

    let mut matched = Vec::new();
    let mut query_ix = 0;
    for (text_ix, ch) in text.chars().enumerate() {
        let text_ch = ch.to_lowercase().next().unwrap_or(ch);
        if text_ch == query[query_ix] {
            matched.push(text_ix);
            query_ix += 1;
            if query_ix == query.len() {
                return Some(matched);
            }
        }
    }
    None
}

fn item_has_search_title(item: &Item) -> bool {
    !item.text.trim().is_empty()
}

fn item_detail(item: &Item, time_format: TimeFormat) -> (String, SearchHitStatus) {
    let (kind, dt) = match item.kind() {
        ItemKind::Event => ("Event", item.start),
        ItemKind::Reminder => ("At", item.start),
        ItemKind::Assignment => ("Due", item.end),
        ItemKind::Procedure => ("Task", None),
    };
    let Some(dt) = dt else {
        return (kind.to_string(), SearchHitStatus::None);
    };
    let local = dt.with_timezone(&Local);
    let status = match item.kind() {
        ItemKind::Event => SearchHitStatus::Event {
            start: dt,
            end: item.end,
        },
        ItemKind::Reminder | ItemKind::Assignment => SearchHitStatus::Date { dt },
        ItemKind::Procedure => SearchHitStatus::None,
    };
    (
        format!(
            "{} {} {}",
            kind,
            local.format("%a"),
            format_time(time_format, local)
        ),
        status,
    )
}
