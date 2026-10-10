use chrono::{DateTime, Duration, Local, NaiveDate, Utc};
use knotq_date_util::format_time;
use knotq_model::{Item, ItemId, ItemKind, Scheme, SchemeId, TimeFormat, Workspace};

use super::text_match::{match_fields, FieldMatch, SearchTerms, Words};
use crate::IndexedWorkspace;

const MAX_SEARCH_HITS: usize = 60;
/// Guessed matches (typos, abbreviations) are only worth showing when little
/// matched what was actually typed.
const LITERAL_HITS_THAT_SILENCE_GUESSES: usize = 5;

// A place to go outranks a line that matches equally well: typing a scheme's
// name is almost always asking for the scheme.
const NAVIGATION_BONUS: i32 = 300;
const SCHEME_BONUS: i32 = 250;

// What a line is tells us how likely it is to be the one wanted, independent
// of how well its text matches. These are deliberately smaller than the gap
// between match kinds, so they order equals rather than override relevance.
const DONE_PENALTY: i32 = 120;
const CURRENT_DATE_BONUS: i32 = 60;
const STALE_DATE_PENALTY: i32 = 40;
const DAILY_TODAY_BONUS: i32 = 40;
const DAILY_THIS_WEEK_BONUS: i32 = 20;
const DAILY_OLD_PENALTY: i32 = 60;

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
    search_hits_at(workspace, time_format, query, options, Utc::now())
}

/// [`search_hits`] with the clock passed in, since ranking prefers what is
/// current.
pub fn search_hits_at(
    workspace: &Workspace,
    time_format: TimeFormat,
    query: &str,
    options: SearchOptions<'_>,
    now: DateTime<Utc>,
) -> Vec<SearchHit> {
    let terms = SearchTerms::new(query);
    let mut candidates = Vec::new();

    push_navigation_candidates(&mut candidates, &terms, options);
    push_scheme_candidates(&mut candidates, workspace, time_format, &terms, now);
    push_daily_queue_candidates(
        &mut candidates,
        workspace,
        time_format,
        &terms,
        options,
        now,
    );

    let literal = candidates.iter().filter(|hit| hit.literal).count();
    if literal >= LITERAL_HITS_THAT_SILENCE_GUESSES {
        candidates.retain(|hit| hit.literal);
    }
    // Stable, so equal scores keep workspace order.
    candidates.sort_by_key(|hit| std::cmp::Reverse(hit.score));
    candidates.truncate(MAX_SEARCH_HITS);
    candidates
        .into_iter()
        .map(|candidate| candidate.source.into_hit(time_format, options))
        .collect()
}

/// A match, held as borrows until it is known to be one of the few shown: a
/// short query matches most of a workspace, and only the top of that list is
/// worth building a [`SearchHit`] for.
#[derive(Clone, Copy, Debug)]
struct Candidate<'a> {
    source: Source<'a>,
    score: i32,
    literal: bool,
}

#[derive(Clone, Copy, Debug)]
enum Source<'a> {
    Calendar,
    DailyQueueView,
    Scheme(&'a Scheme),
    SchemeItem(&'a Scheme, &'a Item),
    DailyQueueItem(&'a Scheme, &'a Item),
}

impl Source<'_> {
    fn into_hit(self, time_format: TimeFormat, options: SearchOptions<'_>) -> SearchHit {
        match self {
            Source::Calendar => SearchHit {
                target: SearchTarget::Calendar,
                scheme_name: "Navigation".to_string(),
                color_index: None,
                color_override: None,
                title: "Calendar".to_string(),
                detail: "view".to_string(),
                status: SearchHitStatus::None,
            },
            Source::DailyQueueView => SearchHit {
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
            },
            Source::Scheme(scheme) => SearchHit {
                target: SearchTarget::Scheme {
                    scheme_id: scheme.id,
                    item_id: None,
                },
                scheme_name: scheme.name.clone(),
                color_index: Some(scheme.color_index),
                color_override: None,
                title: scheme.name.clone(),
                detail: "scheme".to_string(),
                status: SearchHitStatus::None,
            },
            Source::SchemeItem(scheme, item) => {
                let (detail, status) = item_detail(item, time_format);
                SearchHit {
                    target: SearchTarget::Scheme {
                        scheme_id: scheme.id,
                        item_id: Some(item.id),
                    },
                    scheme_name: scheme.name.clone(),
                    color_index: Some(scheme.color_index),
                    color_override: None,
                    title: item.text(),
                    detail,
                    status,
                }
            }
            Source::DailyQueueItem(scheme, item) => SearchHit {
                target: SearchTarget::DailyQueue {
                    scheme_id: Some(scheme.id),
                    item_id: Some(item.id),
                },
                scheme_name: options.daily_queue_title.to_string(),
                color_index: None,
                color_override: Some(options.daily_queue_marker_color),
                title: item.text(),
                detail: item_detail(item, time_format).0,
                status: SearchHitStatus::DailyQueue,
            },
        }
    }
}

fn push_navigation_candidates<'a>(
    candidates: &mut Vec<Candidate<'a>>,
    terms: &SearchTerms,
    options: SearchOptions<'_>,
) {
    if let Some(mat) = match_fields(terms, "Calendar", &terms.words_of("Calendar"), || []) {
        push_candidate(candidates, Source::Calendar, mat, NAVIGATION_BONUS);
    }
    let title = options.daily_queue_title;
    if let Some(mat) = match_fields(terms, title, &terms.words_of(title), || []) {
        push_candidate(candidates, Source::DailyQueueView, mat, NAVIGATION_BONUS);
    }
}

fn push_scheme_candidates<'a>(
    candidates: &mut Vec<Candidate<'a>>,
    workspace: &'a Workspace,
    time_format: TimeFormat,
    terms: &SearchTerms,
    now: DateTime<Utc>,
) {
    let mut title_words = Words::default();
    for scheme in workspace
        .iter_schemes()
        .filter(|scheme| !workspace.is_daily_queue_scheme(scheme.id))
    {
        let scheme_words = terms.words_of(&scheme.name);
        if let Some(mat) = match_fields(terms, &scheme.name, &scheme_words, || []) {
            push_candidate(candidates, Source::Scheme(scheme), mat, SCHEME_BONUS);
        }

        for item in &scheme.items {
            let Some(title) = search_title(item) else {
                continue;
            };
            terms.fill(&mut title_words, title);
            let Some(mat) = match_fields(terms, title, &title_words, || {
                [
                    scheme_words.clone(),
                    terms.words_of(&item_detail(item, time_format).0),
                ]
            }) else {
                continue;
            };
            push_candidate(
                candidates,
                Source::SchemeItem(scheme, item),
                mat,
                item_currency(item, now),
            );
        }
    }
}

fn push_daily_queue_candidates<'a>(
    candidates: &mut Vec<Candidate<'a>>,
    workspace: &'a Workspace,
    time_format: TimeFormat,
    terms: &SearchTerms,
    options: SearchOptions<'_>,
    now: DateTime<Utc>,
) {
    let today = now.with_timezone(&Local).date_naive();
    let daily_queue_words = terms.words_of(options.daily_queue_title);
    let mut title_words = Words::default();
    // Newest day first, so that among equally good matches the recent one leads.
    let mut days: Vec<_> = workspace.iter_daily_queue_schemes().collect();
    days.sort_by_key(|(date, _)| std::cmp::Reverse(*date));
    for (date, scheme) in days {
        let day_currency = daily_queue_day_currency(date, today);
        for item in &scheme.items {
            let Some(title) = search_title(item) else {
                continue;
            };
            terms.fill(&mut title_words, title);
            let Some(mat) = match_fields(terms, title, &title_words, || {
                [
                    daily_queue_words.clone(),
                    terms.words_of(&format!("{}", date.format("%Y %B %-d"))),
                    terms.words_of(&item_detail(item, time_format).0),
                ]
            }) else {
                continue;
            };
            push_candidate(
                candidates,
                Source::DailyQueueItem(scheme, item),
                mat,
                item_currency(item, now) + day_currency,
            );
        }
    }
}

fn push_candidate<'a>(
    candidates: &mut Vec<Candidate<'a>>,
    source: Source<'a>,
    mat: FieldMatch,
    adjustment: i32,
) {
    candidates.push(Candidate {
        source,
        score: mat.score + adjustment,
        literal: mat.literal,
    });
}

/// How much more or less likely a line is to be the one wanted because of its
/// state: finished work and long-past dates sink, what is due around now rises.
fn item_currency(item: &Item, now: DateTime<Utc>) -> i32 {
    if item.repeats.is_none() && item.single_state().is_done() {
        return -DONE_PENALTY;
    }
    let Some(date) = item.start.or(item.end) else {
        return 0;
    };
    if item.repeats.is_some() {
        return 0;
    }
    if date >= now - Duration::days(1) && date <= now + Duration::days(14) {
        CURRENT_DATE_BONUS
    } else if date < now - Duration::days(30) {
        -STALE_DATE_PENALTY
    } else {
        0
    }
}

fn daily_queue_day_currency(date: NaiveDate, today: NaiveDate) -> i32 {
    match (today - date).num_days() {
        ..=0 => DAILY_TODAY_BONUS,
        1..=7 => DAILY_THIS_WEEK_BONUS,
        8..=30 => 0,
        _ => -DAILY_OLD_PENALTY,
    }
}

/// The text a line is found by, or `None` for a line with none (blank, image,
/// table).
fn search_title(item: &Item) -> Option<&str> {
    item.content
        .as_text()
        .filter(|text| !text.trim().is_empty())
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
