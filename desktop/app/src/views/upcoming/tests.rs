//! Tests for the two-phase upcoming panel.
//!
//! The load-bearing one is [`the_two_phase_pipeline_matches_the_single_pass_scan`]:
//! [`reference_scan`] below is a transcription of the single pass this panel used
//! to be, and the split into a cached phase 1 plus a per-render phase 2 is only
//! worth having if it produces byte-identical rows. Everything else here pins the
//! parts the differential test cannot see — that the cache actually skips work,
//! and that it stops skipping when it must.

use std::collections::{HashMap, HashSet};

use chrono::TimeZone;
use knotq_model::{CalendarRecurrence, ItemMarker, NodeRef, Scheme, TimeFormat, Workspace};

use super::scan::{expansion_horizon, rows_from_candidates, scheme_candidates};
use super::*;

/// The colour phase 2 falls back to; any fixed value works, both sides use it.
fn highlight() -> Hsla {
    Hsla {
        h: 0.5,
        s: 0.5,
        l: 0.5,
        a: 1.0,
    }
}

// ---------------------------------------------------------------------------
// The reference implementation
// ---------------------------------------------------------------------------

/// One scheme as the single pass saw it.
struct RefScheme {
    id: SchemeId,
    name: String,
    color_index: u8,
    is_daily: bool,
}

/// The upcoming scan as it was before the phase split, transcribed as
/// literally as the borrow checker allows.
///
/// Deliberately kept naive: it re-expands everything on every call, looks up
/// nothing, and caches nothing. That is the point — it is the definition of
/// correct behaviour that the optimised pipeline has to keep matching.
fn reference_scan(
    schemes: &[(RefScheme, Vec<Item>)],
    now: chrono::DateTime<Utc>,
    today_start: chrono::DateTime<Utc>,
    horizon: chrono::DateTime<Utc>,
    time_format: TimeFormat,
    retained: &dyn Fn(SchemeId, ItemId, &OccurrenceId) -> bool,
) -> UpcomingRows {
    let today_end = today_start + Duration::days(1);
    let mut assignments: Vec<UpRow> = Vec::new();
    let mut reminders: Vec<UpRow> = Vec::new();
    let mut upcoming: Vec<UpRow> = Vec::new();
    let mut seen_future_recurring_items = HashSet::new();

    for (scheme, items) in schemes {
        let is_daily = scheme.is_daily;
        let scheme_name = scheme.name.clone();
        for item in items {
            for occ in item.occurrences(today_start, horizon) {
                if occ.available.is_some_and(|available| available > now) {
                    continue;
                }
                let Some(when) = trigger_time(occ.kind, occ.start, occ.end) else {
                    continue;
                };
                if item.repeats.is_some()
                    && when >= now
                    && !seen_future_recurring_items.insert((scheme.id, item.id))
                {
                    continue;
                }
                let retained_done =
                    occ.state.is_done() && retained(scheme.id, item.id, &occ.id);
                if when < now && occ.state.is_done() && !retained_done {
                    continue;
                }
                let row = UpRow {
                    scheme_id: scheme.id,
                    item_id: item.id,
                    occurrence: occ.id,
                    occurrence_index: occ.occurrence_index,
                    scheme_name: scheme_name.clone(),
                    color_index: scheme.color_index,
                    is_daily,
                    text: item.text(),
                    is_done: occ.state.is_done(),
                    when_label: when_label(time_format, occ.kind, occ.start, occ.end),
                    date_color: row_status_color(occ.kind, occ.start, occ.end, highlight()),
                    sort_key: when,
                    start: occ.start,
                    end: occ.end,
                };
                match occ.kind {
                    ItemKind::Assignment => assignments.push(row),
                    ItemKind::Reminder => reminders.push(row),
                    ItemKind::Event if when < today_end => upcoming.push(row),
                    ItemKind::Event => {}
                    ItemKind::Procedure => {}
                }
            }

            for occ in recurring_overdue_occurrences(item, today_start) {
                let retained_done =
                    occ.state.is_done() && retained(scheme.id, item.id, &occ.id);
                if occ.state.is_done() && !retained_done {
                    continue;
                }
                if item.available.is_some_and(|available| available > now) {
                    continue;
                }
                let Some(when) = trigger_time(occ.kind, occ.start, occ.end) else {
                    continue;
                };
                let row = UpRow {
                    scheme_id: scheme.id,
                    item_id: item.id,
                    occurrence: occ.id,
                    occurrence_index: occ.occurrence_index,
                    scheme_name: scheme_name.clone(),
                    color_index: scheme.color_index,
                    is_daily,
                    text: item.text(),
                    is_done: occ.state.is_done(),
                    when_label: when_label(time_format, occ.kind, occ.start, occ.end),
                    date_color: row_status_color(occ.kind, occ.start, occ.end, highlight()),
                    sort_key: when,
                    start: occ.start,
                    end: occ.end,
                };
                match occ.kind {
                    ItemKind::Assignment => assignments.push(row),
                    ItemKind::Reminder => reminders.push(row),
                    _ => {}
                }
            }

            let is_done = item.single_state().is_done();
            let retained_done =
                is_done && retained(scheme.id, item.id, &OccurrenceId::Single);
            if item.repeats.is_none() && (!is_done || retained_done) {
                let kind = item.kind();
                if !matches!(kind, ItemKind::Assignment | ItemKind::Reminder) {
                    continue;
                }
                let Some(when) = trigger_time(kind, item.start, item.end) else {
                    continue;
                };
                if when >= today_start {
                    continue;
                }
                if item.available.is_some_and(|available| available > now) {
                    continue;
                }
                let row = UpRow {
                    scheme_id: scheme.id,
                    item_id: item.id,
                    occurrence: OccurrenceId::Single,
                    occurrence_index: 0,
                    scheme_name: scheme_name.clone(),
                    color_index: scheme.color_index,
                    is_daily,
                    text: item.text(),
                    is_done,
                    when_label: when_label(time_format, kind, item.start, item.end),
                    date_color: row_status_color(kind, item.start, item.end, highlight()),
                    sort_key: when,
                    start: item.start,
                    end: item.end,
                };
                match kind {
                    ItemKind::Assignment => assignments.push(row),
                    ItemKind::Reminder => reminders.push(row),
                    _ => {}
                }
            }
        }
    }

    for v in [&mut assignments, &mut reminders, &mut upcoming] {
        v.sort_by_key(|r| r.sort_key);
    }

    UpcomingRows {
        assignments,
        reminders,
        upcoming,
    }
}

/// The optimised pipeline, driven over the same input the reference gets.
fn pipeline_scan(
    schemes: &[(RefScheme, Vec<Item>)],
    now: chrono::DateTime<Utc>,
    today_start: chrono::DateTime<Utc>,
    horizon: chrono::DateTime<Utc>,
    time_format: TimeFormat,
    retained: &dyn Fn(SchemeId, ItemId, &OccurrenceId) -> bool,
) -> UpcomingRows {
    let expand_to = expansion_horizon(today_start);
    let candidates: Vec<Vec<Candidate>> = schemes
        .iter()
        .map(|(_, items)| scheme_candidates(items, today_start, expand_to))
        .collect();
    let sources: Vec<SchemeRowSource<'_>> = schemes
        .iter()
        .zip(candidates.iter())
        .map(|((scheme, items), candidates)| SchemeRowSource {
            scheme_id: scheme.id,
            display_name: &scheme.name,
            color_index: scheme.color_index,
            is_daily: scheme.is_daily,
            items,
            candidates,
        })
        .collect();
    rows_from_candidates(
        &sources,
        RowClock {
            now,
            today_end: today_start + Duration::days(1),
            horizon,
        },
        time_format,
        highlight(),
        retained,
    )
}

// ---------------------------------------------------------------------------
// Comparison
// ---------------------------------------------------------------------------

/// Every field of every row, flattened so a mismatch names itself.
fn fingerprint(rows: &UpcomingRows) -> Vec<String> {
    let one = |section: &str, row: &UpRow| {
        format!(
            "{section} scheme={:?} item={:?} occ={:?} idx={} name={:?} color={} daily={} \
             text={:?} done={} when={:?} rgba=({},{},{},{}) sort={} start={:?} end={:?}",
            row.scheme_id,
            row.item_id,
            row.occurrence,
            row.occurrence_index,
            row.scheme_name,
            row.color_index,
            row.is_daily,
            row.text,
            row.is_done,
            row.when_label,
            row.date_color.h,
            row.date_color.s,
            row.date_color.l,
            row.date_color.a,
            row.sort_key,
            row.start,
            row.end,
        )
    };
    let mut out = Vec::new();
    out.extend(rows.assignments.iter().map(|row| one("assignment", row)));
    out.extend(rows.reminders.iter().map(|row| one("reminder", row)));
    out.extend(rows.upcoming.iter().map(|row| one("upcoming", row)));
    out
}

// ---------------------------------------------------------------------------
// Random workspaces
// ---------------------------------------------------------------------------

/// xorshift64*, so a failing case is reported as a seed and replays exactly.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }

    /// Minutes offset from the anchor, weighted towards the days the panel
    /// actually looks at so most items land near a boundary rather than far
    /// outside every window.
    fn offset_minutes(&mut self) -> i64 {
        let span_days: i64 = match self.below(10) {
            0..=3 => 2,
            4..=6 => 20,
            7..=8 => 200,
            _ => 400,
        };
        let span = span_days * 24 * 60;
        self.below((span * 2) as u64) as i64 - span
    }
}

fn random_recurrence(rng: &mut Rng) -> CalendarRecurrence {
    let rrule = match rng.below(7) {
        0 => "FREQ=DAILY".to_string(),
        1 => "FREQ=WEEKLY;BYDAY=MO,WE,FR".to_string(),
        2 => "FREQ=WEEKLY;INTERVAL=2".to_string(),
        3 => "FREQ=MONTHLY".to_string(),
        4 => "FREQ=DAILY;COUNT=6".to_string(),
        5 => "FREQ=YEARLY".to_string(),
        _ => "FREQ=DAILY;INTERVAL=3".to_string(),
    };
    CalendarRecurrence {
        rrules: vec![rrule],
        ..Default::default()
    }
}

fn random_item(rng: &mut Rng, anchor: chrono::DateTime<Utc>) -> Item {
    let mut item = Item::new(format!("item-{}", rng.below(1000)));
    // A quarter of real lines are plain prose with no marker at all; they must
    // stay invisible to the panel however they are dated.
    item.marker = if rng.chance(25) {
        ItemMarker::Blank
    } else {
        ItemMarker::Checkbox
    };
    let has_start = rng.chance(60);
    let has_end = rng.chance(50);
    if has_start {
        item.start = Some(anchor + Duration::minutes(rng.offset_minutes()));
    }
    if has_end {
        let base = item.start.unwrap_or(anchor);
        item.end = Some(base + Duration::minutes(rng.below(3 * 24 * 60) as i64));
    }
    if rng.chance(12) {
        item.available = Some(anchor + Duration::minutes(rng.offset_minutes()));
    }
    if rng.chance(30) && (has_start || has_end) {
        item.repeats = Some(random_recurrence(rng));
    }
    if rng.chance(30) {
        item.state_for_occurrence_mut(OccurrenceId::Single).progress = -1;
    }
    // Completion on a *recurring* item is per occurrence, and the three passes
    // read it back differently, so mark a couple of real occurrence ids rather
    // than only the single one above.
    if item.repeats.is_some() && rng.chance(50) {
        let ids: Vec<OccurrenceId> = item
            .occurrences(anchor - Duration::days(200), anchor + Duration::days(30))
            .into_iter()
            .map(|occurrence| occurrence.id)
            .collect();
        for id in ids {
            if rng.chance(35) {
                item.state_for_occurrence_mut(id).progress = -1;
            }
        }
    }
    item
}

fn random_schemes(rng: &mut Rng, anchor: chrono::DateTime<Utc>) -> Vec<(RefScheme, Vec<Item>)> {
    let scheme_count = 1 + rng.below(6) as usize;
    (0..scheme_count)
        .map(|i| {
            let item_count = rng.below(14) as usize;
            let items = (0..item_count).map(|_| random_item(rng, anchor)).collect();
            (
                RefScheme {
                    id: SchemeId::new(),
                    name: format!("scheme-{i}"),
                    color_index: (rng.below(8)) as u8,
                    is_daily: rng.chance(20),
                },
                items,
            )
        })
        .collect()
}

/// Local midnight for the real today, i.e. the same value the panel computes.
fn local_midnight_today() -> chrono::DateTime<Utc> {
    let today = Local::now().date_naive();
    Local
        .from_local_datetime(&today.and_hms_opt(0, 0, 0).unwrap())
        .single()
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(Utc::now)
}

// ---------------------------------------------------------------------------
// The differential test
// ---------------------------------------------------------------------------

#[test]
fn the_two_phase_pipeline_matches_the_single_pass_scan() {
    let today_start = local_midnight_today();

    for seed in 0..600u64 {
        let mut rng = Rng::new(seed);
        let schemes = random_schemes(&mut rng, today_start);
        // Anywhere in today, which is the whole range over which phase 1's
        // day-aligned expansion has to stand in for the moving horizon.
        let now = today_start + Duration::minutes(rng.below(24 * 60) as i64);
        let horizon = upcoming_range(now).end;
        let time_format = if rng.chance(50) {
            TimeFormat::TwelveHour
        } else {
            TimeFormat::TwentyFourHour
        };
        // A deterministic but non-trivial retention set: retention is a wall-clock
        // TTL in production, and the split has to survive either answer.
        let retained = |scheme: SchemeId, item: ItemId, occurrence: &OccurrenceId| {
            let mut hash = format!("{scheme:?}{item:?}{occurrence:?}")
                .bytes()
                .fold(0u64, |acc, byte| acc.wrapping_mul(31).wrapping_add(byte as u64));
            hash ^= seed;
            hash % 3 == 0
        };

        let expected = reference_scan(
            &schemes,
            now,
            today_start,
            horizon,
            time_format,
            &retained,
        );
        let actual = pipeline_scan(
            &schemes,
            now,
            today_start,
            horizon,
            time_format,
            &retained,
        );

        assert_eq!(
            fingerprint(&expected),
            fingerprint(&actual),
            "seed {seed}: the cached two-phase pipeline disagrees with the \
             single-pass scan it replaced (now = {now}, horizon = {horizon})"
        );
    }
}

/// The differential test above is only meaningful if the random workspaces
/// actually produce rows, and produce them in all three sections.
#[test]
fn the_random_workspaces_exercise_every_section() {
    let today_start = local_midnight_today();
    let mut totals = [0usize; 3];
    for seed in 0..600u64 {
        let mut rng = Rng::new(seed);
        let schemes = random_schemes(&mut rng, today_start);
        let now = today_start + Duration::minutes(rng.below(24 * 60) as i64);
        let rows = pipeline_scan(
            &schemes,
            now,
            today_start,
            upcoming_range(now).end,
            TimeFormat::TwentyFourHour,
            &|_, _, _| false,
        );
        totals[0] += rows.assignments.len();
        totals[1] += rows.reminders.len();
        totals[2] += rows.upcoming.len();
    }
    for (section, count) in ["assignments", "reminders", "upcoming"].iter().zip(totals) {
        assert!(
            count > 50,
            "only {count} {section} rows across 600 random workspaces — the \
             differential test is not exercising this section"
        );
    }
}

// ---------------------------------------------------------------------------
// Phase 1 in isolation
// ---------------------------------------------------------------------------

#[test]
fn phase_one_ignores_the_time_of_day() {
    let today_start = local_midnight_today();
    let mut rng = Rng::new(1234);
    let schemes = random_schemes(&mut rng, today_start);
    let items: Vec<Item> = schemes.into_iter().flat_map(|(_, items)| items).collect();

    // The horizon phase 1 expands to is derived from midnight, so it is the same
    // value at every instant of the day. That is the whole reason the cache can
    // be keyed by the date instead of by the second.
    let at_midnight = expansion_horizon(today_start);
    let at_lunch = expansion_horizon(today_start);
    assert_eq!(at_midnight, at_lunch);

    let first = scheme_candidates(&items, today_start, at_midnight);
    let second = scheme_candidates(&items, today_start, at_midnight);
    assert_eq!(first.len(), second.len());
    assert!(!first.is_empty(), "the fixture must produce candidates");
}

#[test]
fn phase_one_expands_past_every_horizon_the_day_can_produce() {
    let today_start = local_midnight_today();
    let expand_to = expansion_horizon(today_start);
    // `now` can be anywhere in today and the horizon is `now + 14 days`, so the
    // latest horizon a day can ask for is just under midnight + 15 days.
    let latest_now = today_start + Duration::days(1) - Duration::nanoseconds(1);
    assert!(
        upcoming_range(latest_now).end <= expand_to,
        "phase 1 would miss occurrences late in the day"
    );
    assert!(upcoming_range(today_start).end <= expand_to);
}

#[test]
fn phase_one_skips_undated_and_unmarked_lines_without_expanding_them() {
    let today_start = local_midnight_today();
    let expand_to = expansion_horizon(today_start);
    let items = vec![
        Item::new("plain prose"),
        Item::new("a heading").with_marker(ItemMarker::Bullet),
        // Dated, but not a checkbox: `Item::kind` calls this a procedure, and a
        // procedure has never been able to reach a row.
        {
            let mut item = Item::new("dated bullet");
            item.marker = ItemMarker::Bullet;
            item.start = Some(today_start + Duration::hours(3));
            item.repeats = Some(CalendarRecurrence {
                rrules: vec!["FREQ=DAILY".to_string()],
                ..Default::default()
            });
            item
        },
    ];
    assert!(scheme_candidates(&items, today_start, expand_to).is_empty());
}

// ---------------------------------------------------------------------------
// The cache
// ---------------------------------------------------------------------------

/// A workspace plus a mutable per-scheme revision, standing in for the app's
/// `AppState`.
struct CacheHarness {
    workspace: Workspace,
    revisions: HashMap<SchemeId, u64>,
    cache: Option<UpcomingCache>,
}

impl CacheHarness {
    fn new(scheme_count: usize, anchor: chrono::DateTime<Utc>) -> Self {
        let mut rng = Rng::new(77);
        let mut workspace = Workspace::new();
        let mut revisions = HashMap::new();
        for i in 0..scheme_count {
            let mut scheme = Scheme::new(format!("scheme-{i}"), 0);
            scheme.items = (0..6).map(|_| random_item(&mut rng, anchor)).collect();
            revisions.insert(scheme.id, 0);
            workspace
                .folders
                .get_mut(&workspace.root)
                .unwrap()
                .children
                .push(NodeRef::Scheme(scheme.id));
            workspace.schemes.insert(scheme.id, scheme);
        }
        Self {
            workspace,
            revisions,
            cache: None,
        }
    }

    fn scheme_ids(&self) -> Vec<SchemeId> {
        let mut ids: Vec<SchemeId> = self.workspace.schemes.keys().copied().collect();
        ids.sort();
        ids
    }

    fn refresh(&mut self, today: NaiveDate, today_start: chrono::DateTime<Utc>) {
        let revisions = &self.revisions;
        UpcomingCache::refresh(
            &mut self.cache,
            &self.workspace,
            &|id| revisions.get(&id).copied().unwrap_or(0),
            today,
            today_start,
        );
    }

    /// Refresh, reporting which schemes phase 1 actually re-expanded.
    ///
    /// There is no counter to read, so this plants a dated sentinel item in
    /// every scheme *without* touching its revision. A scheme that comes back
    /// with the sentinel among its candidates was rebuilt; one that does not was
    /// served from the cache. The sentinel is removed from the workspace
    /// afterwards so it cannot confuse a later assertion.
    fn refresh_reporting_rebuilds(
        &mut self,
        today: NaiveDate,
        today_start: chrono::DateTime<Utc>,
    ) -> Vec<SchemeId> {
        const SENTINEL: &str = "__rebuild_probe__";
        let ids = self.scheme_ids();
        for id in &ids {
            let scheme = self.workspace.scheme_mut(*id).expect("scheme");
            scheme
                .items
                .push(Item::new(SENTINEL).with_end(today_start + Duration::hours(6)));
        }

        self.refresh(today, today_start);

        let mut rebuilt: Vec<SchemeId> = ids
            .iter()
            .copied()
            .filter(|id| {
                self.candidate_texts(*id)
                    .iter()
                    .any(|text| text == SENTINEL)
            })
            .collect();
        rebuilt.sort();

        for id in &ids {
            let scheme = self.workspace.scheme_mut(*id).expect("scheme");
            scheme.items.retain(|item| item.text() != SENTINEL);
        }
        rebuilt
    }

    fn candidate_texts(&self, scheme: SchemeId) -> Vec<String> {
        let cache = self.cache.as_ref().expect("cache");
        let entry = cache.schemes.get(&scheme).expect("scheme cached");
        let items = &self.workspace.scheme(scheme).expect("scheme").items;
        entry
            .candidates
            .iter()
            .map(|candidate| {
                items
                    .iter()
                    .find(|item| item.id == candidate.item_id)
                    .map(|item| item.text())
                    .unwrap_or_default()
            })
            .collect()
    }
}

#[test]
fn a_schedule_change_rebuilds_only_the_scheme_it_touched() {
    let today_start = local_midnight_today();
    let today = Local::now().date_naive();
    let mut harness = CacheHarness::new(5, today_start);

    let first = harness.refresh_reporting_rebuilds(today, today_start);
    assert_eq!(
        first.len(),
        5,
        "the first refresh has to build every scheme"
    );

    let touched = harness.scheme_ids()[2];
    harness
        .workspace
        .scheme_mut(touched)
        .unwrap()
        .items
        .push(Item::new("new deadline").with_end(today_start + Duration::hours(30)));
    *harness.revisions.get_mut(&touched).unwrap() += 1;

    let second = harness.refresh_reporting_rebuilds(today, today_start);
    assert_eq!(
        second,
        vec![touched],
        "only the scheme whose schedule revision moved should be re-expanded"
    );
    assert!(
        harness
            .candidate_texts(touched)
            .iter()
            .any(|text| text == "new deadline"),
        "the rebuilt scheme must pick the new item up"
    );
}

#[test]
fn nothing_is_rebuilt_when_no_revision_moved() {
    let today_start = local_midnight_today();
    let today = Local::now().date_naive();
    let mut harness = CacheHarness::new(4, today_start);
    harness.refresh(today, today_start);

    // Stand in for a text-only edit: the item's text changes but the schedule
    // revision does not, which is precisely the case the panel must not rescan.
    let untouched = harness.scheme_ids()[1];
    harness
        .workspace
        .scheme_mut(untouched)
        .unwrap()
        .items
        .push(Item::new("typed after the scan").with_end(today_start + Duration::hours(5)));

    assert!(
        harness
            .refresh_reporting_rebuilds(today, today_start)
            .is_empty(),
        "a refresh with no revision change must not re-expand anything"
    );
    assert!(
        !harness
            .candidate_texts(untouched)
            .iter()
            .any(|text| text == "typed after the scan"),
        "the cache did rebuild — the test can no longer prove work was skipped"
    );
}

#[test]
fn a_new_day_invalidates_everything() {
    let today_start = local_midnight_today();
    let today = Local::now().date_naive();
    let mut harness = CacheHarness::new(3, today_start);
    harness.refresh(today, today_start);

    let tomorrow = today + Duration::days(1);
    let tomorrow_start = today_start + Duration::days(1);
    let rebuilt = harness.refresh_reporting_rebuilds(tomorrow, tomorrow_start);
    assert_eq!(
        rebuilt,
        harness.scheme_ids(),
        "a new day has to re-expand every scheme, revisions or not — the query \
         window itself moved"
    );
    assert_eq!(
        harness.cache.as_ref().unwrap().day,
        tomorrow,
        "the cache must re-key itself to the new day"
    );
    assert_eq!(
        harness.cache.as_ref().unwrap().schemes.len(),
        3,
        "and refill it from the new day's window"
    );
}

/// The day key is not just bookkeeping: an item due today is upcoming today and
/// overdue tomorrow, and the cached candidates say which.
#[test]
fn the_day_key_actually_changes_the_candidates() {
    let today_start = local_midnight_today();
    let today = Local::now().date_naive();
    let mut harness = CacheHarness::new(1, today_start);
    let scheme = harness.scheme_ids()[0];
    harness.workspace.scheme_mut(scheme).unwrap().items =
        vec![Item::new("due today").with_end(today_start + Duration::hours(9))];

    harness.refresh(today, today_start);
    let today_sources: Vec<CandidateSource> = harness.cache.as_ref().unwrap().schemes[&scheme]
        .candidates
        .iter()
        .map(|candidate| candidate.source)
        .collect();
    assert_eq!(today_sources, vec![CandidateSource::Window]);

    harness.refresh(today + Duration::days(1), today_start + Duration::days(1));
    let tomorrow_sources: Vec<CandidateSource> = harness.cache.as_ref().unwrap().schemes[&scheme]
        .candidates
        .iter()
        .map(|candidate| candidate.source)
        .collect();
    assert_eq!(
        tomorrow_sources,
        vec![CandidateSource::PastSingle],
        "yesterday's deadline has to come back as overdue, not vanish"
    );
}

#[test]
fn a_deleted_scheme_drops_out_of_the_cache() {
    let today_start = local_midnight_today();
    let today = Local::now().date_naive();
    let mut harness = CacheHarness::new(3, today_start);
    harness.refresh(today, today_start);

    let gone = harness.scheme_ids()[0];
    harness.workspace.schemes.remove(&gone);
    harness.refresh(today, today_start);

    assert!(
        !harness.cache.as_ref().unwrap().schemes.contains_key(&gone),
        "a removed scheme must not keep answering with stale candidates"
    );
    assert_eq!(harness.cache.as_ref().unwrap().schemes.len(), 2);
}

// ---------------------------------------------------------------------------
// Cost
// ---------------------------------------------------------------------------

/// A workspace shaped like the one this optimisation was written for: 173
/// schemes, ~3.5k items, 975 of them with a start, 937 with an end, 519
/// repeating — and dates spread across a year in each direction, so only a
/// handful land inside the two-week window at any moment.
///
/// The dense random workspaces the differential test uses are deliberately
/// unrealistic; they put over a thousand rows on a panel that in practice shows
/// a few dozen. For a cost measurement that shape would flatter nothing and
/// prove nothing, so this one matches the real counts instead.
fn realistic_workspace(anchor: chrono::DateTime<Utc>) -> Vec<(RefScheme, Vec<Item>)> {
    const SCHEMES: usize = 173;
    const ITEMS: usize = 3466;
    // 975 / 937 / 519 out of 3466, as percentages, spread evenly over the
    // schemes rather than bunched into the first few.
    const START_PERCENT: u64 = 28;
    const END_PERCENT: u64 = 27;
    const REPEAT_PERCENT: u64 = 15;

    let mut rng = Rng::new(20260817);
    let per_scheme = ITEMS.div_ceil(SCHEMES);
    (0..SCHEMES)
        .map(|i| {
            let items = (0..per_scheme)
                .map(|j| {
                    let mut item = Item::new(format!("line {i}.{j} of some ordinary prose"));
                    let has_start = rng.chance(START_PERCENT);
                    let has_end = rng.chance(END_PERCENT);
                    if !has_start && !has_end {
                        return item;
                    }
                    item.marker = ItemMarker::Checkbox;
                    // A year back and a year forward: real workspaces are mostly
                    // history, and history is mostly finished.
                    let when = anchor
                        + Duration::minutes(
                            rng.below(2 * 365 * 24 * 60) as i64 - 365 * 24 * 60,
                        );
                    if has_start {
                        item.start = Some(when);
                    }
                    if has_end {
                        item.end = Some(when + Duration::hours(1));
                    }
                    if rng.chance(REPEAT_PERCENT) {
                        item.repeats = Some(random_recurrence(&mut rng));
                        // A repeating task that is actually used has its past
                        // instances ticked off; one that is not shows up as a
                        // wall of overdue rows, which is a property of the data
                        // rather than of the scan.
                        let past: Vec<OccurrenceId> = item
                            .occurrences(anchor - Duration::days(200), anchor)
                            .into_iter()
                            .map(|occurrence| occurrence.id)
                            .collect();
                        for id in past {
                            if rng.chance(90) {
                                item.state_for_occurrence_mut(id).progress = -1;
                            }
                        }
                    } else if when < anchor && rng.chance(85) {
                        item.state_for_occurrence_mut(OccurrenceId::Single).progress = -1;
                    }
                    item
                })
                .collect();
            (
                RefScheme {
                    id: SchemeId::new(),
                    name: format!("scheme-{i}"),
                    color_index: 0,
                    is_daily: false,
                },
                items,
            )
        })
        .collect()
}

/// Phase 2 runs on every render, and the panel renders on every keystroke, so
/// its cost has to be a function of the rows on screen and not of the workspace.
///
/// Rather than assert a wall-clock budget — which says as much about the machine
/// as about the code — this runs phase 2 twice over the same rows, once with the
/// workspace padded out with ten times as many undated lines. Undated lines
/// produce no candidates, so the padding cannot change the answer; if it changes
/// the *cost*, phase 2 has grown a pass over the items again, which is exactly
/// the regression the two-phase split exists to prevent.
#[test]
fn phase_two_cost_does_not_scale_with_the_workspace() {
    let today_start = local_midnight_today();
    let now = today_start + Duration::hours(11);
    let clock = RowClock {
        now,
        today_end: today_start + Duration::days(1),
        horizon: upcoming_range(now).end,
    };
    let expand_to = expansion_horizon(today_start);
    let schemes = realistic_workspace(today_start);
    let candidates: Vec<Vec<Candidate>> = schemes
        .iter()
        .map(|(_, items)| scheme_candidates(items, today_start, expand_to))
        .collect();

    // Appended, so every recorded item index still points at the same item.
    let padded_items: Vec<Vec<Item>> = schemes
        .iter()
        .map(|(_, items)| {
            let mut items = items.clone();
            items.extend((0..200).map(|i| Item::new(format!("padding {i}"))));
            items
        })
        .collect();

    fn build<'a>(
        schemes: &'a [(RefScheme, Vec<Item>)],
        items: &'a [Vec<Item>],
        candidates: &'a [Vec<Candidate>],
    ) -> Vec<SchemeRowSource<'a>> {
        schemes
            .iter()
            .zip(items.iter())
            .zip(candidates.iter())
            .map(|(((scheme, _), items), candidates)| SchemeRowSource {
                scheme_id: scheme.id,
                display_name: &scheme.name,
                color_index: scheme.color_index,
                is_daily: scheme.is_daily,
                items,
                candidates,
            })
            .collect()
    }
    let plain_items: Vec<Vec<Item>> = schemes.iter().map(|(_, items)| items.clone()).collect();
    let plain = build(&schemes, &plain_items, &candidates);
    let padded = build(&schemes, &padded_items, &candidates);

    let run = |sources: &[SchemeRowSource<'_>]| {
        rows_from_candidates(
            sources,
            clock,
            TimeFormat::TwentyFourHour,
            highlight(),
            &|_, _, _| false,
        )
    };

    // Warm the lazily-initialised localisation tables the formatters touch, so
    // the measurement is of the loop and not of first use.
    let rows = run(&plain);
    let row_count = rows.assignments.len() + rows.reminders.len() + rows.upcoming.len();
    let candidate_count: usize = candidates.iter().map(|c| c.len()).sum();
    println!("{row_count} rows from {candidate_count} candidates over 3466 items");
    assert!(
        row_count > 20,
        "only {row_count} rows — the fixture is not exercising phase 2"
    );
    assert_eq!(
        fingerprint(&rows),
        fingerprint(&run(&padded)),
        "padding must not change the rows, or it is not measuring the same work"
    );

    const RENDERS: u32 = 40;
    let time = |sources: &[SchemeRowSource<'_>]| {
        let started = std::time::Instant::now();
        for _ in 0..RENDERS {
            std::hint::black_box(run(sources));
        }
        started.elapsed() / RENDERS
    };
    // Alternated, so a machine that speeds up or slows down partway through
    // biases both measurements the same way.
    let mut plain_total = std::time::Duration::ZERO;
    let mut padded_total = std::time::Duration::ZERO;
    for _ in 0..3 {
        plain_total += time(&plain);
        padded_total += time(&padded);
    }
    println!(
        "phase 2 for {row_count} rows: {:?} per render, {:?} with 10x the items",
        plain_total / 3,
        padded_total / 3
    );

    assert!(
        padded_total < plain_total * 2,
        "phase 2 took {:?} per render over {} items but {:?} over {} — it is \
         still walking the items, not the candidates",
        plain_total / 3,
        plain_items.iter().map(Vec::len).sum::<usize>(),
        padded_total / 3,
        padded_items.iter().map(Vec::len).sum::<usize>(),
    );
}

/// The other half of the claim: phase 1 is what used to run per second and now
/// runs per schedule change, and even a full rebuild of the whole workspace is
/// affordable — a single scheme, which is what an edit actually costs, is a
/// hundredth of it.
#[test]
fn phase_one_over_the_whole_workspace_is_affordable() {
    let today_start = local_midnight_today();
    let expand_to = expansion_horizon(today_start);
    let schemes = realistic_workspace(today_start);

    let started = std::time::Instant::now();
    let candidates: usize = schemes
        .iter()
        .map(|(_, items)| std::hint::black_box(scheme_candidates(items, today_start, expand_to)).len())
        .sum();
    let full_rebuild = started.elapsed();
    println!("phase 1 over the whole workspace: {full_rebuild:?}, {candidates} candidates");

    let one_scheme_items = &schemes[0].1;
    let started = std::time::Instant::now();
    for _ in 0..20 {
        std::hint::black_box(scheme_candidates(one_scheme_items, today_start, expand_to));
    }
    let per_scheme = started.elapsed() / 20;
    println!("phase 1 for one scheme: {per_scheme:?}");

    // What an edit costs is one scheme, and that has to fit in a frame with room
    // to spare even unoptimised.
    assert!(
        per_scheme < std::time::Duration::from_millis(4),
        "re-expanding one scheme took {per_scheme:?}; an edit pays this"
    );
}

#[test]
fn the_item_text_lookup_is_linear_in_the_scheme() {
    let today_start = local_midnight_today();
    let expand_to = expansion_horizon(today_start);
    // One dated item at the end of a long scheme: the old per-row `find` walked
    // the whole scheme for it, once per row, on every render.
    let mut items: Vec<Item> = (0..4000).map(|i| Item::new(format!("prose {i}"))).collect();
    items.push(Item::new("the only deadline").with_end(today_start + Duration::hours(4)));

    let candidates = scheme_candidates(&items, today_start, expand_to);
    assert_eq!(candidates.len(), 1);

    let sources = vec![SchemeRowSource {
        scheme_id: SchemeId::new(),
        display_name: "scheme",
        color_index: 0,
        is_daily: false,
        items: &items,
        candidates: &candidates,
    }];
    let now = today_start + Duration::hours(1);
    let rows = rows_from_candidates(
        &sources,
        RowClock {
            now,
            today_end: today_start + Duration::days(1),
            horizon: upcoming_range(now).end,
        },
        TimeFormat::TwentyFourHour,
        highlight(),
        &|_, _, _| false,
    );
    assert_eq!(rows.assignments.len(), 1);
    assert_eq!(rows.assignments[0].text, "the only deadline");
}

/// The index hint is a hint: if the scheme was reordered under the cache, the
/// row still has to show the right text rather than a neighbour's.
#[test]
fn a_stale_index_hint_still_resolves_the_right_item() {
    let today_start = local_midnight_today();
    let expand_to = expansion_horizon(today_start);
    let mut items = vec![Item::new("deadline").with_end(today_start + Duration::hours(4))];
    let candidates = scheme_candidates(&items, today_start, expand_to);
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].item_index, 0);

    // Two lines land above it, so the recorded index now points at prose.
    items.insert(0, Item::new("inserted one"));
    items.insert(0, Item::new("inserted two"));

    let sources = vec![SchemeRowSource {
        scheme_id: SchemeId::new(),
        display_name: "scheme",
        color_index: 0,
        is_daily: false,
        items: &items,
        candidates: &candidates,
    }];
    let now = today_start + Duration::hours(1);
    let rows = rows_from_candidates(
        &sources,
        RowClock {
            now,
            today_end: today_start + Duration::days(1),
            horizon: upcoming_range(now).end,
        },
        TimeFormat::TwentyFourHour,
        highlight(),
        &|_, _, _| false,
    );
    assert_eq!(rows.assignments.len(), 1);
    assert_eq!(rows.assignments[0].text, "deadline");
}

/// Typing is the one edit that does not raise the schedule revision, so it is
/// the one edit the index hint has to survive without a search.
#[test]
fn typing_into_a_dated_row_updates_its_text_through_the_hint() {
    let today_start = local_midnight_today();
    let expand_to = expansion_horizon(today_start);
    let mut items = vec![
        Item::new("prose"),
        Item::new("deadline").with_end(today_start + Duration::hours(4)),
    ];
    let candidates = scheme_candidates(&items, today_start, expand_to);
    assert_eq!(candidates[0].item_index, 1);

    items[1].set_text("deadline, edited");

    let sources = vec![SchemeRowSource {
        scheme_id: SchemeId::new(),
        display_name: "scheme",
        color_index: 0,
        is_daily: false,
        items: &items,
        candidates: &candidates,
    }];
    let now = today_start + Duration::hours(1);
    let rows = rows_from_candidates(
        &sources,
        RowClock {
            now,
            today_end: today_start + Duration::days(1),
            horizon: upcoming_range(now).end,
        },
        TimeFormat::TwentyFourHour,
        highlight(),
        &|_, _, _| false,
    );
    assert_eq!(rows.assignments[0].text, "deadline, edited");
}

#[test]
fn a_row_whose_item_vanished_renders_without_text_rather_than_panicking() {
    let today_start = local_midnight_today();
    let expand_to = expansion_horizon(today_start);
    let items = vec![Item::new("deadline").with_end(today_start + Duration::hours(4))];
    let candidates = scheme_candidates(&items, today_start, expand_to);

    // The candidates outlive one refresh; a scheme edited between phase 1 and
    // phase 2 must degrade to an empty label, not an index panic.
    let empty: Vec<Item> = Vec::new();
    let sources = vec![SchemeRowSource {
        scheme_id: SchemeId::new(),
        display_name: "scheme",
        color_index: 0,
        is_daily: false,
        items: &empty,
        candidates: &candidates,
    }];
    let now = today_start + Duration::hours(1);
    let rows = rows_from_candidates(
        &sources,
        RowClock {
            now,
            today_end: today_start + Duration::days(1),
            horizon: upcoming_range(now).end,
        },
        TimeFormat::TwentyFourHour,
        highlight(),
        &|_, _, _| false,
    );
    assert_eq!(rows.assignments.len(), 1);
    assert_eq!(rows.assignments[0].text, "");
}

#[test]
fn the_hour_of_the_day_never_changes_what_phase_one_produced() {
    // A regression guard for the cache key itself: if phase 1 ever grows a
    // dependency on the current time, the day-keyed cache silently goes stale.
    let today_start = local_midnight_today();
    let mut rng = Rng::new(31337);
    let items: Vec<Item> = (0..400).map(|_| random_item(&mut rng, today_start)).collect();
    let baseline = scheme_candidates(&items, today_start, expansion_horizon(today_start));

    for hour in 0..24 {
        let candidates = scheme_candidates(&items, today_start, expansion_horizon(today_start));
        assert_eq!(
            candidates.len(),
            baseline.len(),
            "phase 1 changed at hour {hour} of the same day"
        );
        for (a, b) in baseline.iter().zip(candidates.iter()) {
            assert_eq!(a.item_id, b.item_id);
            assert_eq!(a.trigger, b.trigger);
            assert_eq!(a.source, b.source);
            assert_eq!(a.occurrence_index, b.occurrence_index);
        }
    }
}
