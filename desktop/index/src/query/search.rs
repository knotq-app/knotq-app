use chrono::Local;
use knotq_date_util::format_time;
use knotq_model::{Item, ItemId, ItemKind, SchemeId, TimeFormat, Workspace};
use std::cmp::Ordering;

use crate::IndexedWorkspace;

const MAX_SEARCH_HITS: usize = 60;
const NAVIGATION_FIELD_WEIGHT: i32 = 5_000;
const SCHEME_TITLE_FIELD_WEIGHT: i32 = 4_600;
const ITEM_TITLE_FIELD_WEIGHT: i32 = 4_000;
const SCHEME_CONTEXT_FIELD_WEIGHT: i32 = 1_200;
const DAILY_QUEUE_CONTEXT_FIELD_WEIGHT: i32 = 1_100;
const DETAIL_FIELD_WEIGHT: i32 = 800;

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

    hits.sort_by(compare_ranked_hits);
    hits.truncate(MAX_SEARCH_HITS);
    hits.into_iter().map(|hit| hit.hit).collect()
}

#[derive(Clone, Debug)]
struct RankedSearchHit {
    hit: SearchHit,
    rank: SearchRank,
    ordinal: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SearchRank {
    score: i32,
    start: usize,
    span: usize,
}

#[derive(Clone, Copy, Debug)]
struct TextMatch {
    score: i32,
    start: usize,
    span: usize,
}

#[derive(Clone, Debug)]
struct TokenSpan {
    text: String,
    start: usize,
    end: usize,
}

fn push_navigation_hits(hits: &mut Vec<RankedSearchHit>, query: &str, options: SearchOptions<'_>) {
    if let Some(rank) = field_rank("Calendar", query, NAVIGATION_FIELD_WEIGHT) {
        push_ranked_hit(
            hits,
            SearchHit {
                target: SearchTarget::Calendar,
                scheme_name: "Navigation".to_string(),
                color_index: None,
                color_override: None,
                title: "Calendar".to_string(),
                detail: "view".to_string(),
                status: SearchHitStatus::None,
            },
            rank,
        );
    }
    if let Some(rank) = field_rank(options.daily_queue_title, query, NAVIGATION_FIELD_WEIGHT) {
        push_ranked_hit(
            hits,
            SearchHit {
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
            rank,
        );
    }
}

fn push_scheme_hits(
    hits: &mut Vec<RankedSearchHit>,
    workspace: &Workspace,
    time_format: TimeFormat,
    query: &str,
) {
    for scheme in workspace
        .iter_schemes()
        .filter(|scheme| !workspace.is_daily_queue_scheme(scheme.id))
    {
        if let Some(rank) = field_rank(&scheme.name, query, SCHEME_TITLE_FIELD_WEIGHT) {
            push_ranked_hit(
                hits,
                SearchHit {
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
                rank,
            );
        }

        for item in &scheme.items {
            if !item_has_search_title(item) {
                continue;
            }
            let (detail, status) = item_detail(item, time_format);
            let Some(rank) = best_rank([
                field_rank(&item.text, query, ITEM_TITLE_FIELD_WEIGHT),
                field_rank(&scheme.name, query, SCHEME_CONTEXT_FIELD_WEIGHT),
                field_rank(&detail, query, DETAIL_FIELD_WEIGHT),
            ]) else {
                continue;
            };

            push_ranked_hit(
                hits,
                SearchHit {
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
                },
                rank,
            );
        }
    }
}

fn push_daily_queue_hits(
    hits: &mut Vec<RankedSearchHit>,
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
            let Some(rank) = best_rank([
                field_rank(&item.text, query, ITEM_TITLE_FIELD_WEIGHT),
                field_rank(
                    options.daily_queue_title,
                    query,
                    DAILY_QUEUE_CONTEXT_FIELD_WEIGHT,
                ),
                field_rank(&day_label, query, DAILY_QUEUE_CONTEXT_FIELD_WEIGHT),
                field_rank(&detail, query, DETAIL_FIELD_WEIGHT),
            ]) else {
                continue;
            };

            push_ranked_hit(
                hits,
                SearchHit {
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
                },
                rank,
            );
        }
    }
}

fn push_ranked_hit(hits: &mut Vec<RankedSearchHit>, hit: SearchHit, rank: SearchRank) {
    hits.push(RankedSearchHit {
        hit,
        rank,
        ordinal: hits.len(),
    });
}

fn compare_ranked_hits(left: &RankedSearchHit, right: &RankedSearchHit) -> Ordering {
    right
        .rank
        .score
        .cmp(&left.rank.score)
        .then_with(|| left.rank.start.cmp(&right.rank.start))
        .then_with(|| left.rank.span.cmp(&right.rank.span))
        .then_with(|| left.ordinal.cmp(&right.ordinal))
}

fn best_rank<const N: usize>(ranks: [Option<SearchRank>; N]) -> Option<SearchRank> {
    ranks
        .into_iter()
        .flatten()
        .max_by(|left, right| compare_ranks(left, right))
}

fn compare_ranks(left: &SearchRank, right: &SearchRank) -> Ordering {
    left.score
        .cmp(&right.score)
        .then_with(|| right.start.cmp(&left.start))
        .then_with(|| right.span.cmp(&left.span))
}

fn field_rank(text: &str, query: &str, field_weight: i32) -> Option<SearchRank> {
    text_match(text, query).map(|mat| SearchRank {
        score: field_weight + mat.score,
        start: mat.start,
        span: mat.span,
    })
}

fn text_match(text: &str, query: &str) -> Option<TextMatch> {
    let query = query.trim();
    if query.is_empty() {
        return Some(TextMatch {
            score: 0,
            start: 0,
            span: 0,
        });
    }

    let text = text.trim();
    if text.is_empty() {
        return None;
    }

    let query_lower = query.to_lowercase();
    let text_lower = text.to_lowercase();
    let query_len = query_lower.chars().count();
    let text_len = text_lower.chars().count();
    let mut best = None;

    if text_lower == query_lower {
        keep_better_match(
            &mut best,
            TextMatch {
                score: 1_250,
                start: 0,
                span: text_len,
            },
        );
    }

    if let Some(byte_ix) = text_lower.find(&query_lower) {
        let start = text_lower[..byte_ix].chars().count();
        let boundary_bonus = if is_word_boundary(&text_lower, byte_ix) {
            90
        } else {
            0
        };
        let prefix_bonus = if start == 0 { 110 } else { 0 };
        keep_better_match(
            &mut best,
            TextMatch {
                score: penalized_score(
                    880 + boundary_bonus + prefix_bonus,
                    start,
                    query_len,
                    query_len,
                ),
                start,
                span: query_len,
            },
        );
    }

    if let Some(mat) = token_match(&text_lower, &query_lower) {
        keep_better_match(&mut best, mat);
    }

    if let Some(indices) = subsequence_match_indices(text, query) {
        if let (Some(first), Some(last)) = (indices.first(), indices.last()) {
            let span = last.saturating_sub(*first) + 1;
            keep_better_match(
                &mut best,
                TextMatch {
                    score: penalized_score(520, *first, span, query_len),
                    start: *first,
                    span,
                },
            );
        }
    }

    best
}

fn token_match(text_lower: &str, query_lower: &str) -> Option<TextMatch> {
    let text_tokens = token_spans(text_lower);
    let query_tokens = token_spans(query_lower);
    if text_tokens.is_empty() || query_tokens.is_empty() {
        return None;
    }

    let mut text_ix = 0;
    let mut first_start = None;
    let mut last_end = 0;
    let mut quality_sum = 0;

    for query_token in &query_tokens {
        let mut best_token_ix = None;
        let mut best_quality = 0;

        for (ix, token) in text_tokens.iter().enumerate().skip(text_ix) {
            let quality = token_match_quality(&token.text, &query_token.text);
            if quality == 0 {
                continue;
            }
            if quality > best_quality {
                best_token_ix = Some(ix);
                best_quality = quality;
            }
            if quality >= 120 {
                break;
            }
        }

        let token_ix = best_token_ix?;
        let token = &text_tokens[token_ix];
        first_start.get_or_insert(token.start);
        last_end = token.end;
        quality_sum += best_quality;
        text_ix = token_ix + 1;
    }

    let start = first_start.unwrap_or(0);
    let span = last_end.saturating_sub(start);
    let query_len: usize = query_tokens
        .iter()
        .map(|token| token.text.chars().count())
        .sum();
    let average_quality = quality_sum / query_tokens.len() as i32;
    let base = if query_tokens.len() == 1 { 720 } else { 780 };

    Some(TextMatch {
        score: penalized_score(base + average_quality, start, span, query_len),
        start,
        span,
    })
}

fn token_match_quality(token: &str, query: &str) -> i32 {
    if token == query {
        120
    } else if token.starts_with(query) {
        100
    } else if token.contains(query) {
        70
    } else {
        0
    }
}

fn token_spans(text: &str) -> Vec<TokenSpan> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut start = 0;

    for (ix, ch) in text.chars().enumerate() {
        if ch.is_alphanumeric() {
            if current.is_empty() {
                start = ix;
            }
            current.extend(ch.to_lowercase());
            continue;
        }
        if !current.is_empty() {
            tokens.push(TokenSpan {
                text: std::mem::take(&mut current),
                start,
                end: ix,
            });
        }
    }

    if !current.is_empty() {
        tokens.push(TokenSpan {
            text: current,
            start,
            end: text.chars().count(),
        });
    }

    tokens
}

fn is_word_boundary(text: &str, byte_ix: usize) -> bool {
    if byte_ix == 0 {
        return true;
    }
    text[..byte_ix]
        .chars()
        .next_back()
        .is_none_or(|ch| !ch.is_alphanumeric())
}

fn penalized_score(base: i32, start: usize, span: usize, query_len: usize) -> i32 {
    let start_penalty = start.min(80) as i32 * 2;
    let gap_penalty = span.saturating_sub(query_len).min(120) as i32 * 5;
    (base - start_penalty - gap_penalty).max(1)
}

fn keep_better_match(best: &mut Option<TextMatch>, candidate: TextMatch) {
    let replace = best.as_ref().is_none_or(|best| {
        candidate
            .score
            .cmp(&best.score)
            .then_with(|| best.start.cmp(&candidate.start))
            .then_with(|| best.span.cmp(&candidate.span))
            == Ordering::Greater
    });
    if replace {
        *best = Some(candidate);
    }
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
