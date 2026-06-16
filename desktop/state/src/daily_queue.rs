use std::collections::HashSet;

use chrono::{DateTime, Datelike, Duration, Local, LocalResult, NaiveDate, TimeZone, Utc};
use knotq_commands::Command;
use knotq_model::{Item, ItemId, ItemMarker, NodeRef, Scheme, SchemeId, Workspace};

#[derive(Clone, Debug)]
pub struct DailyQueueState {
    pub today: NaiveDate,
    pub loaded_start: NaiveDate,
    pub visible_dates: HashSet<NaiveDate>,
    pub loaded_calendar_months: HashSet<(i32, u32)>,
}

impl DailyQueueState {
    pub fn new(today: NaiveDate, loaded_start: NaiveDate) -> Self {
        Self {
            today,
            loaded_start,
            visible_dates: HashSet::new(),
            loaded_calendar_months: HashSet::new(),
        }
    }

    pub fn sync_day_boundary(&mut self, today: NaiveDate) -> bool {
        if self.today == today {
            return false;
        }
        self.today = today;
        true
    }
}

pub fn daily_queue_scheme_name(date: NaiveDate) -> String {
    format!("Daily {}", date.format("%Y-%m-%d"))
}

pub fn daily_queue_carryover_command(
    previous_id: SchemeId,
    previous: &Scheme,
    today_id: SchemeId,
    today: &Scheme,
) -> Option<Command> {
    if daily_queue_scheme_is_blank(previous) {
        return None;
    }

    let mut commands = Vec::new();
    let mut carried_items = Vec::new();

    for item in &previous.items {
        if daily_queue_item_is_fully_complete_task(item) {
            continue;
        }

        let mut carried = item.clone();
        carried.id = ItemId::new();
        carried_items.push(carried);

        if daily_queue_item_has_annotations(item) {
            let mut stripped = item.clone();
            strip_daily_queue_annotations(&mut stripped);
            commands.push(Command::ReplaceItem {
                scheme: previous_id,
                item: stripped,
            });
        }
    }

    if carried_items.is_empty() {
        return None;
    }

    let mut position = today.items.len();
    if daily_queue_scheme_is_blank(today) && !today.items.is_empty() {
        let mut first = carried_items.remove(0);
        first.id = today.items[0].id;
        commands.push(Command::ReplaceItem {
            scheme: today_id,
            item: first,
        });
        position = 1;
    }

    for item in carried_items {
        commands.push(Command::InsertItem {
            scheme: today_id,
            position,
            item,
        });
        position += 1;
    }

    (!commands.is_empty()).then_some(Command::Batch(commands))
}

/// How many days back the daily-queue carryover ("roll over") scans for the most
/// recent day with content. Two weeks bridges a weekend or a short break while
/// keeping the lookback bounded — and stays within the span the app preloads on
/// open, so the candidate schemes are already in memory.
pub const DAILY_QUEUE_CARRYOVER_LOOKBACK_DAYS: i64 = 14;

/// The most recent daily-queue day strictly before `today` (within
/// [`DAILY_QUEUE_CARRYOVER_LOOKBACK_DAYS`]) that has carryable content. Returns
/// `None` when every day in that window is blank or absent. Only consults
/// schemes already loaded in `workspace`; days whose scheme bytes haven't been
/// paged in are treated as having nothing to carry.
pub fn last_nonempty_daily_queue_day(workspace: &Workspace, today: NaiveDate) -> Option<NaiveDate> {
    (1..=DAILY_QUEUE_CARRYOVER_LOOKBACK_DAYS)
        .map(|offset| today - Duration::days(offset))
        .find(|date| {
            workspace
                .daily_queue_scheme_id(*date)
                .and_then(|id| workspace.scheme(id))
                .is_some_and(|scheme| !daily_queue_scheme_is_blank(scheme))
        })
}

pub fn daily_queue_scheme_is_blank(scheme: &Scheme) -> bool {
    if scheme.items.is_empty() {
        return true;
    }
    scheme
        .items
        .first()
        .is_some_and(daily_queue_item_is_blank_placeholder)
        && scheme.items.len() == 1
}

pub fn make_default_workspace() -> Workspace {
    make_default_workspace_for_date(Local::now().date_naive())
}

pub fn make_default_workspace_for_date(today: NaiveDate) -> Workspace {
    let mut workspace = Workspace::new();
    insert_root_scheme(&mut workspace, make_start_here_scheme(today));
    insert_root_scheme(&mut workspace, make_scheduling_scheme(today));
    insert_root_scheme(&mut workspace, make_projects_scheme(today));

    let yesterday = today - Duration::days(1);
    let mut past_daily = Scheme::new(
        daily_queue_scheme_name(yesterday),
        knotq_model::DAILY_QUEUE_COLOR_INDEX,
    );
    past_daily.id = knotq_model::daily_queue_scheme_id(yesterday);
    past_daily.items = vec![
        Item::new("Past daily pages stay available here"),
        Item::new("Completed tasks stay behind as a record").done(),
    ];
    let past_daily_id = past_daily.id;
    workspace.daily_queue.insert(yesterday, past_daily_id);
    workspace.schemes.insert(past_daily_id, past_daily);
    workspace.scheme_sync.insert(
        past_daily_id,
        knotq_model::daily_queue_sync_metadata(yesterday),
    );

    let mut daily = Scheme::new(
        daily_queue_scheme_name(today),
        knotq_model::DAILY_QUEUE_COLOR_INDEX,
    );
    daily.id = knotq_model::daily_queue_scheme_id(today);
    daily.items = make_daily_seed_items();
    let daily_id = daily.id;
    workspace.daily_queue.insert(today, daily_id);
    workspace.schemes.insert(daily_id, daily);
    workspace
        .scheme_sync
        .insert(daily_id, knotq_model::daily_queue_sync_metadata(today));
    workspace
}

fn insert_root_scheme(workspace: &mut Workspace, scheme: Scheme) {
    let scheme_id = scheme.id;
    workspace.schemes.insert(scheme_id, scheme);
    if let Some(root) = workspace.folders.get_mut(&workspace.root) {
        root.children.push(NodeRef::Scheme(scheme_id));
    }
}

fn make_start_here_scheme(today: NaiveDate) -> Scheme {
    let mut scheme = Scheme::new("Coursework", 0);
    scheme.items = vec![
        Item::new("History Paper"),
        Item::new("Thesis: compare the economic causes of two revolutions")
            .with_marker(ItemMarker::Bullet),
        Item::new("Submit final draft")
            .with_marker(ItemMarker::Checkbox)
            .with_end(local_dt(today + Duration::days(2), 17, 0)),
        Item::new("Finish source notes")
            .with_marker(ItemMarker::Checkbox)
            .done(),
        Item::new("Add two quotes from chapter 4")
            .with_marker(ItemMarker::Checkbox)
            .with_indent(1),
        Item::new("Ask about citation format after class")
            .with_marker(ItemMarker::Checkbox)
            .with_start(local_dt(today + Duration::days(1), 15, 30)),
        Item::new("Exam Prep"),
        Item::new("Review lecture notes from weeks 3 and 4").with_marker(ItemMarker::Checkbox),
        Item::new("Make flashcards for key terms").with_marker(ItemMarker::Checkbox),
        Item::new("Schedule a study session")
            .with_marker(ItemMarker::Checkbox)
            .with_start(local_dt(today + Duration::days(1), 19, 0))
            .with_end(local_dt(today + Duration::days(1), 20, 30)),
        Item::new("Questions for office hours").with_marker(ItemMarker::Bullet),
    ];
    scheme
}

fn make_scheduling_scheme(today: NaiveDate) -> Scheme {
    let mut scheme = Scheme::new("Scheduling", 5);
    scheme.items = vec![
        Item::new("Four ways a task can relate to time"),
        Item::new("Event: focus block")
            .with_marker(ItemMarker::Checkbox)
            .with_start(local_dt(today, 10, 0))
            .with_end(local_dt(today, 11, 0)),
        Item::new("Start + end gives you an event with a duration on the calendar")
            .with_marker(ItemMarker::Bullet)
            .with_indent(1),
        Item::new("Assignment: submit first draft")
            .with_marker(ItemMarker::Checkbox)
            .with_end(local_dt(today + Duration::days(1), 17, 0)),
        Item::new("No start + end gives you a deadline")
            .with_marker(ItemMarker::Bullet)
            .with_indent(1),
        Item::new("Reminder: message the team")
            .with_marker(ItemMarker::Checkbox)
            .with_start(local_dt(today, 16, 0)),
        Item::new("Start + no end gives you a reminder at a specific time")
            .with_marker(ItemMarker::Bullet)
            .with_indent(1),
        Item::new("Backlog: choose next experiment").with_marker(ItemMarker::Checkbox),
        Item::new("No start + no end gives you a normal task that stays out of the calendar")
            .with_marker(ItemMarker::Bullet)
            .with_indent(1),
    ];
    scheme
}

fn make_projects_scheme(today: NaiveDate) -> Scheme {
    let mut scheme = Scheme::new("Projects", 3);
    scheme.items = vec![
        Item::new("Break large work into specific next actions"),
        Item::new("Draft the outline")
            .with_marker(ItemMarker::Checkbox)
            .done(),
        Item::new("Review open questions")
            .with_marker(ItemMarker::Checkbox)
            .with_end(local_dt(today + Duration::days(2), 12, 0)),
        Item::new("Schedule a focused work session")
            .with_marker(ItemMarker::Checkbox)
            .with_start(local_dt(today + Duration::days(1), 14, 0))
            .with_end(local_dt(today + Duration::days(1), 15, 30)),
        Item::new("Nested lines become context for the parent task")
            .with_marker(ItemMarker::Bullet)
            .with_indent(1),
    ];
    scheme
}

fn make_daily_seed_items() -> Vec<Item> {
    vec![
        Item::new("Daily Queue"),
        Item::new("Start each day with an optimistic list of concrete, small tasks"),
        Item::new("Make coffee")
            .with_marker(ItemMarker::Checkbox)
            .done(),
        Item::new("Review today's calendar").with_marker(ItemMarker::Checkbox),
        Item::new("Write the next draft").with_marker(ItemMarker::Checkbox),
        Item::new("Send one follow-up message").with_marker(ItemMarker::Checkbox),
        Item::new(""),
        Item::new("The goal is to avoid deciding what to do next after every task"),
        Item::new("Keep the list realistic, but long enough to last the day")
            .with_marker(ItemMarker::Bullet)
            .with_indent(1),
        Item::new("Use Daily for quick notes and loose ideas too")
            .with_marker(ItemMarker::Bullet)
            .with_indent(1),
        Item::new("Add dates here when a daily task needs time or a deadline")
            .with_marker(ItemMarker::Bullet)
            .with_indent(1),
    ]
}

fn local_dt(date: NaiveDate, hour: u32, minute: u32) -> DateTime<Utc> {
    match Local.with_ymd_and_hms(date.year(), date.month(), date.day(), hour, minute, 0) {
        LocalResult::Single(dt) | LocalResult::Ambiguous(dt, _) => dt.with_timezone(&Utc),
        LocalResult::None => {
            DateTime::from_naive_utc_and_offset(date.and_hms_opt(hour, minute, 0).unwrap(), Utc)
        }
    }
}

fn daily_queue_item_is_fully_complete_task(item: &Item) -> bool {
    item.marker == ItemMarker::Checkbox
        && !item.state.is_empty()
        && item.state.iter().all(|state| state.state.is_done())
}

fn daily_queue_item_has_annotations(item: &Item) -> bool {
    item.start.is_some() || item.end.is_some() || item.available.is_some() || item.repeats.is_some()
}

fn strip_daily_queue_annotations(item: &mut Item) {
    item.start = None;
    item.end = None;
    item.available = None;
    item.repeats = None;
}

fn daily_queue_item_is_blank_placeholder(item: &Item) -> bool {
    item.text.trim().is_empty()
        && item.media.is_empty()
        && item.marker == ItemMarker::Blank
        && item.indent == 0
        && !daily_queue_item_has_annotations(item)
        && item.priority.is_none()
        && item.state.len() == 1
        && item.state[0].state.progress == 0
        && item.state[0].state.notification_offset_secs.is_none()
}
