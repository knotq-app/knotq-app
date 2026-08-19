//! The upcoming panel in two phases.
//!
//! **Phase 1** ([`scheme_candidates`]) turns a scheme into [`Candidate`]s. This
//! is the expensive half: every repeating item is expanded across a 180-day
//! lookback and a two-week horizon, and a scheme's worth of that easily costs
//! more than the rest of a frame. Its result depends only on the scheme's
//! contents and on *which day it is* — never on the current second — so it can
//! be cached per scheme and rebuilt only for the scheme a change actually
//! touched.
//!
//! **Phase 2** ([`rows_from_candidates`]) applies everything that does depend on
//! the wall clock: what is now overdue, what is still hidden behind an
//! `available` time, whether a completed row has aged out of its retention
//! window, and how each date reads at this moment. It runs on every render, but
//! only over the candidates — a couple of thousand on a large workspace, against
//! the tens of thousands of occurrences phase 1 expanded to find them.
//!
//! The split has to be exact, not merely close, or the panel shows something the
//! straightforward one-pass version would not. [`super::tests`] pins that by
//! differential-testing this pipeline against a transcription of the original
//! single pass.
//!
//! On a workspace of 173 schemes / 3466 items / ~500 repeating items, release
//! build: phase 2 is 0.27ms per render and does not move when the item count
//! goes up tenfold; phase 1 is 4.5ms for the whole workspace but 16µs for the
//! one scheme an edit actually touches. The version this replaced re-ran both
//! halves, for every scheme, at least once per second — the cache key included
//! the current second — and again on every non-text command.

use super::*;

/// The end of the window phase 1 expands.
///
/// The real horizon is `now + UPCOMING_HORIZON_DAYS`, which moves every second.
/// Phase 1 rounds it up to the next whole day so its work is stable for a day,
/// and phase 2 re-applies the exact horizon to the handful of candidates that
/// come back. Widening only the *end* of the window is safe: recurrence
/// expansion walks anchors forward from the item's own anchor, so a later end
/// appends occurrences without disturbing the ones already there — including
/// their `occurrence_index`, which is counted in anchor order.
pub(super) fn expansion_horizon(today_start: chrono::DateTime<Utc>) -> chrono::DateTime<Utc> {
    today_start + Duration::days(UPCOMING_HORIZON_DAYS + 1)
}

/// The instant an occurrence starts overlapping a query window that ends at
/// `to`, mirroring `knotq_rrule`'s own range test.
fn enters_window_at(
    start: Option<chrono::DateTime<Utc>>,
    end: Option<chrono::DateTime<Utc>>,
    anchor: chrono::DateTime<Utc>,
) -> chrono::DateTime<Utc> {
    // An occurrence with both ends is in range while it has not started yet
    // ending after the window opens; one with a single end is placed by that end
    // alone. Either way this is the value compared against the window's close.
    match (start, end) {
        (Some(start), Some(_)) => start,
        _ => anchor,
    }
}

fn occurrence_anchor(occurrence: &Occurrence) -> Option<chrono::DateTime<Utc>> {
    occurrence.start.or(occurrence.end).or(occurrence.available)
}

/// Phase 1 for one scheme.
pub(super) fn scheme_candidates(
    items: &[Item],
    today_start: chrono::DateTime<Utc>,
    expand_to: chrono::DateTime<Utc>,
) -> Vec<Candidate> {
    let mut candidates: Vec<Candidate> = Vec::new();

    for (item_index, item) in items.iter().enumerate() {
        // `Item::kind` is a property of the item, and every occurrence it
        // expands to carries that same kind. All three branches below end in a
        // `match` on the kind that drops `Procedure` on the floor, so such an
        // item can never reach a row. The one thing it *could* do in the
        // single-pass original was claim a slot in the future-recurring dedup
        // set — but that set is keyed by the item, whose other occurrences are
        // equally dropped, so the claim was never observable either.
        //
        // Skipping here is therefore the same filter, moved ahead of the
        // recurrence expansion that would otherwise run, and allocate, for
        // every undated line in the workspace. Most lines are undated.
        if item.kind() == ItemKind::Procedure {
            continue;
        }

        for occurrence in item.occurrences(today_start, expand_to) {
            let Some(trigger) = trigger_time(occurrence.kind, occurrence.start, occurrence.end)
            else {
                continue;
            };
            let anchor = occurrence_anchor(&occurrence).unwrap_or(trigger);
            candidates.push(Candidate {
                item_id: item.id,
                item_index,
                occurrence_index: occurrence.occurrence_index,
                kind: occurrence.kind,
                start: occurrence.start,
                end: occurrence.end,
                available: occurrence.available,
                enters_window_at: enters_window_at(occurrence.start, occurrence.end, anchor),
                trigger,
                is_done: occurrence.state.is_done(),
                repeats: item.repeats.is_some(),
                source: CandidateSource::Window,
                occurrence: occurrence.id,
            });
        }

        // The window above starts at midnight, so it cannot include yesterday's
        // recurrence. Keep a bounded set of missed instances alongside it.
        for occurrence in recurring_overdue_occurrences(item, today_start) {
            if !matches!(occurrence.kind, ItemKind::Assignment | ItemKind::Reminder) {
                continue;
            }
            let Some(trigger) = trigger_time(occurrence.kind, occurrence.start, occurrence.end)
            else {
                continue;
            };
            candidates.push(Candidate {
                item_id: item.id,
                item_index,
                occurrence_index: occurrence.occurrence_index,
                kind: occurrence.kind,
                start: occurrence.start,
                end: occurrence.end,
                // The overdue pass gates on the *item's* availability, not the
                // occurrence's, as the single-pass original did.
                available: item.available,
                enters_window_at: trigger,
                trigger,
                is_done: occurrence.state.is_done(),
                repeats: true,
                source: CandidateSource::Overdue,
                occurrence: occurrence.id,
            });
        }

        if item.repeats.is_none() {
            let kind = item.kind();
            if !matches!(kind, ItemKind::Assignment | ItemKind::Reminder) {
                continue;
            }
            let Some(trigger) = trigger_time(kind, item.start, item.end) else {
                continue;
            };
            if trigger >= today_start {
                continue;
            }
            candidates.push(Candidate {
                item_id: item.id,
                item_index,
                occurrence: OccurrenceId::Single,
                occurrence_index: 0,
                kind,
                start: item.start,
                end: item.end,
                available: item.available,
                enters_window_at: trigger,
                trigger,
                is_done: item.single_state().is_done(),
                repeats: false,
                source: CandidateSource::PastSingle,
            });
        }
    }

    candidates
}

/// Phase 2: the clock-dependent filters and formatting.
///
/// `retained` answers whether a completed occurrence is still inside its
/// retention window — itself a function of the current time, which is why
/// completion cannot be resolved in phase 1.
pub(super) fn rows_from_candidates(
    sources: &[SchemeRowSource<'_>],
    clock: RowClock,
    time_format: knotq_model::TimeFormat,
    highlight: Hsla,
    retained: &dyn Fn(SchemeId, ItemId, &OccurrenceId) -> bool,
) -> UpcomingRows {
    let mut assignments: Vec<UpRow> = Vec::new();
    let mut reminders: Vec<UpRow> = Vec::new();
    let mut upcoming: Vec<UpRow> = Vec::new();
    // A repeating item contributes at most one *future* row, whichever comes
    // first. Candidates arrive in the order the single-pass original visited
    // them, so first-wins picks the same occurrence it did.
    let mut seen_future_recurring_items = std::collections::HashSet::new();

    for source in sources {
        if source.candidates.is_empty() {
            continue;
        }
        for candidate in source.candidates {
            let scheme_id = source.scheme_id;
            let is_retained = || {
                candidate.is_done && retained(scheme_id, candidate.item_id, &candidate.occurrence)
            };
            let hidden_until_available = candidate
                .available
                .is_some_and(|available| available > clock.now);

            match candidate.source {
                CandidateSource::Window => {
                    // Phase 1 expanded to the next day boundary; this is the
                    // real horizon.
                    if candidate.enters_window_at >= clock.horizon {
                        continue;
                    }
                    if hidden_until_available {
                        continue;
                    }
                    if candidate.repeats
                        && candidate.trigger >= clock.now
                        && !seen_future_recurring_items.insert((scheme_id, candidate.item_id))
                    {
                        continue;
                    }
                    if candidate.trigger < clock.now && candidate.is_done && !is_retained() {
                        continue;
                    }
                }
                CandidateSource::Overdue => {
                    if candidate.is_done && !is_retained() {
                        continue;
                    }
                    if hidden_until_available {
                        continue;
                    }
                }
                CandidateSource::PastSingle => {
                    if candidate.is_done && !is_retained() {
                        continue;
                    }
                    if hidden_until_available {
                        continue;
                    }
                }
            }

            let row = UpRow {
                scheme_id,
                item_id: candidate.item_id,
                occurrence: candidate.occurrence.clone(),
                occurrence_index: candidate.occurrence_index,
                scheme_name: source.display_name.to_string(),
                color_index: source.color_index,
                is_daily: source.is_daily,
                text: candidate_text(source.items, candidate),
                is_done: candidate.is_done,
                when_label: when_label(time_format, candidate.kind, candidate.start, candidate.end),
                date_color: row_status_color(
                    candidate.kind,
                    candidate.start,
                    candidate.end,
                    highlight,
                ),
                sort_key: candidate.trigger,
                start: candidate.start,
                end: candidate.end,
            };

            match candidate.kind {
                ItemKind::Assignment => assignments.push(row),
                ItemKind::Reminder => reminders.push(row),
                ItemKind::Event
                    if candidate.source == CandidateSource::Window
                        && candidate.trigger < clock.today_end =>
                {
                    upcoming.push(row)
                }
                ItemKind::Event => {}
                ItemKind::Procedure => {}
            }
        }
    }

    // Sort by date only — toggling done should not reshuffle the list, since that
    // makes the row "jump" out from under the user's cursor when they click it.
    for rows in [&mut assignments, &mut reminders, &mut upcoming] {
        rows.sort_by_key(|row| row.sort_key);
    }

    UpcomingRows {
        assignments,
        reminders,
        upcoming,
    }
}

/// Body text for one row.
///
/// Phase 2 runs on every render, so this must not depend on how long the scheme
/// is: resolving each row by scanning its scheme was quadratic in a long scheme,
/// and building a lookup table per scheme was linear in the whole workspace,
/// which is the cost the cache exists to avoid paying per frame.
///
/// So phase 1 records where the item sat and phase 2 checks that it is still
/// there. The only edit that does not raise the schedule revision is
/// `UpdateItemText`, which cannot move an item, so the hint is expected to hold
/// on exactly the path that matters — typing. The search behind it is a
/// correctness backstop for a cache that has somehow outlived its scheme's
/// layout, not a path the panel is meant to take.
fn candidate_text(items: &[Item], candidate: &Candidate) -> String {
    items
        .get(candidate.item_index)
        .filter(|item| item.id == candidate.item_id)
        .or_else(|| items.iter().find(|item| item.id == candidate.item_id))
        .map(|item| item.text())
        .unwrap_or_default()
}

impl KnotQApp {
    /// The panel's rows for this frame.
    ///
    /// Phase 1 is cached per scheme against
    /// [`AppState::scheme_schedule_revision`] and the local date; phase 2 always
    /// runs. The root view re-renders on every keystroke, so what matters is
    /// that neither a text edit nor the clock advancing reaches phase 1.
    pub(super) fn upcoming_rows(&mut self, cx: &mut Context<Self>) -> UpcomingRows {
        let now = Utc::now();
        let today = Local::now().date_naive();
        let today_start = Local
            .from_local_datetime(
                &today
                    .and_hms_opt(0, 0, 0)
                    .unwrap_or_else(|| Local::now().naive_local()),
            )
            .single()
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or(now);
        let horizon = upcoming_range(now).end;
        // Loading a daily-queue range can add schemes, so it runs before the
        // cache is consulted; when it does load something it bumps the schedule
        // revision and invalidates the affected entries on its own.
        self.ensure_daily_queue_calendar_range_loaded(
            today,
            horizon.with_timezone(&Local).date_naive(),
            cx,
        );

        // Named up front so the borrows are visibly of distinct fields: the
        // cache is a field of the app, the workspace and the revisions both live
        // on the state it derefs to.
        let cache_slot = &mut self.upcoming_cache;
        let state = &self.state;
        UpcomingCache::refresh(
            cache_slot,
            &state.workspace,
            &|scheme| state.scheme_schedule_revision(scheme),
            today,
            today_start,
        );

        let clock = RowClock {
            now,
            today_end: today_start + Duration::days(1),
            horizon,
        };
        let cache = self
            .upcoming_cache
            .as_ref()
            .expect("UpcomingCache::refresh always leaves a cache");

        // Deterministic order so rows that share a trigger time keep a stable
        // position between frames; `iter_schemes` walks a hash map.
        let mut schemes: Vec<&knotq_model::Scheme> = self.workspace.iter_schemes().collect();
        schemes.sort_by_key(|scheme| scheme.id);

        let daily_queue: std::collections::HashSet<SchemeId> =
            self.workspace.daily_queue.values().copied().collect();
        let names: Vec<Option<String>> = schemes
            .iter()
            .map(|scheme| {
                let has_rows = cache
                    .schemes
                    .get(&scheme.id)
                    .is_some_and(|entry| !entry.candidates.is_empty());
                has_rows.then(|| {
                    if daily_queue.contains(&scheme.id) {
                        crate::app::DAILY_QUEUE_TITLE.to_string()
                    } else {
                        scheme.name.clone()
                    }
                })
            })
            .collect();

        let sources: Vec<SchemeRowSource<'_>> = schemes
            .iter()
            .zip(names.iter())
            .filter_map(|(scheme, name)| {
                let entry = cache.schemes.get(&scheme.id)?;
                Some(SchemeRowSource {
                    scheme_id: scheme.id,
                    display_name: name.as_deref().unwrap_or_default(),
                    color_index: scheme.color_index,
                    is_daily: daily_queue.contains(&scheme.id),
                    items: &scheme.items,
                    candidates: &entry.candidates,
                })
            })
            .collect();

        let highlight = token_hsla(self.theme().text_highlight);
        let time_format = self.time_format;
        rows_from_candidates(
            &sources,
            clock,
            time_format,
            highlight,
            &|scheme_id, item_id, occurrence| {
                self.retains_completed_calendar_item(scheme_id, item_id, occurrence)
            },
        )
    }
}

impl UpcomingCache {
    /// Bring the phase-1 cache up to date: rebuild the schemes whose schedule
    /// moved, drop the ones that are gone, and start over on a new day.
    ///
    /// Takes the slot rather than `&mut self` so a `None` cache and a cache from
    /// yesterday are the same case, and takes the workspace and the revision
    /// lookup separately so the caller can hand it disjoint borrows of itself.
    pub(super) fn refresh(
        slot: &mut Option<Self>,
        workspace: &knotq_model::Workspace,
        revision_of: &dyn Fn(SchemeId) -> u64,
        today: NaiveDate,
        today_start: chrono::DateTime<Utc>,
    ) {
        let expand_to = expansion_horizon(today_start);
        if slot.as_ref().is_none_or(|cache| cache.day != today) {
            *slot = Some(UpcomingCache::new(today));
        }
        let cache = slot.as_mut().expect("just populated when stale");

        let live: Vec<(SchemeId, u64)> = workspace
            .iter_schemes()
            .map(|scheme| (scheme.id, revision_of(scheme.id)))
            .collect();

        // A deleted scheme must not keep answering with yesterday's rows.
        cache
            .schemes
            .retain(|id, _| live.iter().any(|(live_id, _)| live_id == id));

        for (id, revision) in live {
            let outdated = cache
                .schemes
                .get(&id)
                .is_none_or(|entry| entry.revision != revision);
            if !outdated {
                continue;
            }
            let Some(scheme) = workspace.scheme(id) else {
                continue;
            };
            cache.schemes.insert(
                id,
                SchemeCandidates {
                    revision,
                    candidates: scheme_candidates(&scheme.items, today_start, expand_to),
                },
            );
        }
    }
}
