use std::collections::HashSet;

use chrono::{DateTime, Datelike, Duration, Local, LocalResult, NaiveDate, TimeZone, Utc};
use knotq_commands::Command;
use knotq_model::{
    daily_queue_displaced_item_id, DocumentId, Item, ItemId, ItemMarker, NodeRef, Scheme, SchemeId,
    SyncDocumentKind, SyncDocumentMeta, Workspace,
};

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
    previous_date: NaiveDate,
    previous: &Scheme,
    today_id: SchemeId,
    today: &Scheme,
) -> Option<Command> {
    if daily_queue_scheme_is_blank(previous) {
        return None;
    }

    // The carried row KEEPS the source item's id — notification identity, and any
    // annotation state keyed by item id, follow the live item into today. The row
    // left behind on the source day is re-identified with a deterministic
    // displaced id instead (see `daily_queue_displaced_item_id`). Skipping ids
    // already present in today makes a repeat carryover idempotent: a double
    // click, a retry after a sync clobbered the optimistic insert, or a sync that
    // re-created a just-carried row never inserts a second copy — and the shared
    // ids let the CRDT item-skeleton merge collapse two devices' concurrent
    // carries of the same row into one item instead of doubling it.
    let existing: HashSet<ItemId> = today.items.iter().map(|item| item.id).collect();

    let mut commands = Vec::new();
    let mut carried_items = Vec::new();

    for (source_position, item) in previous.items.iter().enumerate() {
        if daily_queue_item_is_fully_complete_task(item) {
            continue;
        }
        if existing.contains(&item.id) {
            // Already carried into today — skip so re-running carryover is a no-op.
            continue;
        }

        carried_items.push(item.clone());

        // Re-identify the archived source row (its id now lives in today) and
        // strip its date annotations so it stops scheduling anything. ReplaceItem
        // locates rows by id, so an id change needs a delete + insert pair; the
        // pair is position-neutral, so later indices stay valid.
        let mut displaced = item.clone();
        displaced.id = daily_queue_displaced_item_id(item.id, previous_date);
        strip_daily_queue_annotations(&mut displaced);
        commands.push(Command::DeleteItem {
            scheme: previous_id,
            item: item.id,
        });
        commands.push(Command::InsertItem {
            scheme: previous_id,
            position: source_position,
            item: displaced,
        });
    }

    if carried_items.is_empty() {
        return None;
    }

    // When today is just its blank placeholder row, drop that row so carried items
    // don't sit under a leading blank. It is DELETED rather than its id reused for
    // the first carried row, so every carried row keeps its source id.
    let placeholder_id = if daily_queue_scheme_is_blank(today) {
        today.items.first().map(|item| item.id)
    } else {
        None
    };

    let mut position = if placeholder_id.is_some() {
        1
    } else {
        today.items.len()
    };
    for item in carried_items {
        commands.push(Command::InsertItem {
            scheme: today_id,
            position,
            item,
        });
        position += 1;
    }
    if let Some(placeholder_id) = placeholder_id {
        commands.push(Command::DeleteItem {
            scheme: today_id,
            item: placeholder_id,
        });
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
    insert_root_scheme(
        &mut workspace,
        make_start_here_scheme(today),
        fixed_document_id("00000000-0000-8000-8000-000000000201"),
    );
    insert_root_scheme(
        &mut workspace,
        make_scheduling_scheme(today),
        fixed_document_id("00000000-0000-8000-8000-000000000202"),
    );
    insert_root_scheme(
        &mut workspace,
        make_projects_scheme(today),
        fixed_document_id("00000000-0000-8000-8000-000000000203"),
    );

    let yesterday = today - Duration::days(1);
    let mut past_daily = Scheme::new(
        daily_queue_scheme_name(yesterday),
        knotq_model::DAILY_QUEUE_COLOR_INDEX,
    );
    past_daily.id = knotq_model::daily_queue_scheme_id(yesterday);
    past_daily.items = vec![
        fixed_item(
            "00000000-0000-8000-8000-000000000402",
            "Past daily pages stay available here",
        ),
        fixed_item(
            "00000000-0000-8000-8000-000000000403",
            "Completed work stays behind as a record",
        )
        .with_marker(ItemMarker::Checkbox)
        .done(),
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

fn insert_root_scheme(workspace: &mut Workspace, scheme: Scheme, sync_document_id: DocumentId) {
    let scheme_id = scheme.id;
    workspace.schemes.insert(scheme_id, scheme);
    workspace
        .scheme_sync
        .insert(scheme_id, fixed_scheme_sync(sync_document_id));
    if let Some(root) = workspace.folders.get_mut(&workspace.root) {
        root.children.push(NodeRef::Scheme(scheme_id));
    }
}

fn make_start_here_scheme(today: NaiveDate) -> Scheme {
    let mut scheme = Scheme::new("Coursework", 0);
    scheme.id = fixed_scheme_id("00000000-0000-8000-8000-000000000101");
    scheme.items = vec![
        fixed_item("00000000-0000-8000-8000-000000001003", "### Thesis"),
        fixed_item(
            "00000000-0000-8000-8000-000000001004",
            "==Argument:== compare economic pressure and *public trust*",
        )
        .with_marker(ItemMarker::Bullet),
        // The one intentionally-incomplete dated item: an overdue assignment (due
        // yesterday), so new users can see what "overdue" looks like. Past-dated, so
        // it never schedules a notification.
        fixed_item("00000000-0000-8000-8000-000000001005", "Final draft")
            .with_marker(ItemMarker::Checkbox)
            .with_end(local_dt(today - Duration::days(1), 17, 0)),
        fixed_item(
            "00000000-0000-8000-8000-000000001006",
            "Finish source notes",
        )
        .with_marker(ItemMarker::Checkbox)
        .done(),
        fixed_item(
            "00000000-0000-8000-8000-000000001007",
            "Add two primary quotes from chapter 4",
        )
        .with_marker(ItemMarker::Checkbox)
        .with_indent(1),
        fixed_item("00000000-0000-8000-8000-000000001008", "Citation question")
            .with_marker(ItemMarker::Checkbox)
            .with_start(local_dt(today + Duration::days(1), 15, 30))
            .done(),
        fixed_item(
            "00000000-0000-8000-8000-000000001009",
            "Old outline moved into final draft",
        )
        .with_marker(ItemMarker::Bullet)
        .with_indent(1),
        fixed_item("00000000-0000-8000-8000-000000001010", "### Exam Prep"),
        fixed_item(
            "00000000-0000-8000-8000-000000001011",
            "Review **weeks 3-4** lecture notes",
        )
        .with_marker(ItemMarker::Checkbox),
        fixed_item(
            "00000000-0000-8000-8000-000000001012",
            "Make flashcards for *key terms*",
        )
        .with_marker(ItemMarker::Checkbox),
        fixed_item("00000000-0000-8000-8000-000000001013", "Study block")
            .with_marker(ItemMarker::Checkbox)
            .with_start(local_dt(today + Duration::days(1), 19, 0))
            .with_end(local_dt(today + Duration::days(1), 20, 30))
            .done(),
        fixed_item(
            "00000000-0000-8000-8000-000000001014",
            "Questions for office hours",
        )
        .with_marker(ItemMarker::Bullet)
        .with_indent(1),
    ];
    scheme
}

fn make_scheduling_scheme(today: NaiveDate) -> Scheme {
    let mut scheme = Scheme::new("Scheduling", 5);
    scheme.id = fixed_scheme_id("00000000-0000-8000-8000-000000000102");
    scheme.items = vec![
        fixed_item(
            "00000000-0000-8000-8000-000000002002",
            "### Calendar shapes",
        ),
        fixed_item(
            "00000000-0000-8000-8000-000000002003",
            "**Events** have a start and end",
        )
        .with_marker(ItemMarker::Bullet),
        fixed_item("00000000-0000-8000-8000-000000002004", "Focus block")
            .with_marker(ItemMarker::Checkbox)
            .with_start(local_dt(today, 10, 0))
            .with_end(local_dt(today, 11, 0))
            .done(),
        fixed_item(
            "00000000-0000-8000-8000-000000002005",
            "**Assignments** have a deadline",
        )
        .with_marker(ItemMarker::Bullet),
        fixed_item("00000000-0000-8000-8000-000000002006", "First draft")
            .with_marker(ItemMarker::Checkbox)
            .with_end(local_dt(today + Duration::days(1), 17, 0))
            .done(),
        fixed_item(
            "00000000-0000-8000-8000-000000002007",
            "**Reminders** happen at one time",
        )
        .with_marker(ItemMarker::Bullet),
        fixed_item("00000000-0000-8000-8000-000000002008", "Team message")
            .with_marker(ItemMarker::Checkbox)
            .with_start(local_dt(today, 16, 0))
            .done(),
        fixed_item(
            "00000000-0000-8000-8000-000000002009",
            "### Repeating habits",
        ),
        fixed_item("00000000-0000-8000-8000-000000002010", "Morning review")
            .with_marker(ItemMarker::Checkbox)
            .with_start(local_dt(today, 8, 30))
            .done(),
        fixed_item(
            "00000000-0000-8000-8000-000000002011",
            "Backlog: choose next experiment",
        )
        .with_marker(ItemMarker::Checkbox),
    ];
    scheme
}

fn make_projects_scheme(today: NaiveDate) -> Scheme {
    let mut scheme = Scheme::new("Projects", 3);
    scheme.id = fixed_scheme_id("00000000-0000-8000-8000-000000000103");
    scheme.items = vec![
        fixed_item("00000000-0000-8000-8000-000000003001", "### Launch Plan"),
        fixed_item(
            "00000000-0000-8000-8000-000000003003",
            "Make the first screen feel **clear**, *fast*, and alive",
        )
        .with_marker(ItemMarker::Bullet),
        fixed_item("00000000-0000-8000-8000-000000003004", "Draft the outline")
            .with_marker(ItemMarker::Checkbox)
            .done(),
        fixed_item("00000000-0000-8000-8000-000000003005", "Open questions")
            .with_marker(ItemMarker::Checkbox)
            .with_end(local_dt(today + Duration::days(2), 12, 0))
            .done(),
        fixed_item("00000000-0000-8000-8000-000000003006", "Work session")
            .with_marker(ItemMarker::Checkbox)
            .with_start(local_dt(today + Duration::days(1), 14, 0))
            .with_end(local_dt(today + Duration::days(1), 15, 0))
            .done(),
        fixed_item(
            "00000000-0000-8000-8000-000000003007",
            "Nested notes keep context close",
        )
        .with_marker(ItemMarker::Bullet)
        .with_indent(1),
        fixed_item(
            "00000000-0000-8000-8000-000000003008",
            "Risk: ==too much process==; keep the path light",
        )
        .with_marker(ItemMarker::Bullet)
        .with_indent(1),
        fixed_item("00000000-0000-8000-8000-000000003009", "### Design polish"),
        fixed_item(
            "00000000-0000-8000-8000-000000003011",
            "Find the moment that matters",
        )
        .with_marker(ItemMarker::Checkbox),
        fixed_item(
            "00000000-0000-8000-8000-000000003012",
            "Cut rough copy into **sharper labels**",
        )
        .with_marker(ItemMarker::Checkbox),
        fixed_item(
            "00000000-0000-8000-8000-000000003013",
            "Ship a tiny, beautiful default workspace",
        )
        .with_marker(ItemMarker::Checkbox),
    ];
    scheme
}

fn make_daily_seed_items() -> Vec<Item> {
    vec![
        fixed_item("00000000-0000-8000-8000-000000004002", "Make coffee")
            .with_marker(ItemMarker::Checkbox)
            .done(),
        fixed_item(
            "00000000-0000-8000-8000-000000004003",
            "Review today's calendar",
        )
        .with_marker(ItemMarker::Checkbox),
        fixed_item("00000000-0000-8000-8000-000000004004", "Write the next draft")
            .with_marker(ItemMarker::Checkbox),
        fixed_item(
            "00000000-0000-8000-8000-000000004005",
            "Send one **thoughtful** follow-up",
        )
        .with_marker(ItemMarker::Checkbox),
        fixed_item("00000000-0000-8000-8000-000000004006", ""),
        fixed_item(
            "00000000-0000-8000-8000-000000004007",
            "### Daily Recommendations (My workflow)",
        ),
        fixed_item(
            "00000000-0000-8000-8000-000000004011",
            "Write an optimistic list of *concrete, small* tasks at the start of each day and check them as you progress",
        )
        .with_marker(ItemMarker::Numbered)
        .with_indent(1),
        fixed_item(
            "00000000-0000-8000-8000-000000004008",
            "The point is that **you never have to make a decision about what to do next**",
        )
        .with_marker(ItemMarker::Numbered)
        .with_indent(1),
        fixed_item(
            "00000000-0000-8000-8000-000000004009",
            "When you finish a task the next one is right there",
        )
        .with_marker(ItemMarker::Bullet)
        .with_indent(2),
        fixed_item(
            "00000000-0000-8000-8000-000000004010",
            "KnotQ can carry over incomplete items to the next day; completed work stays behind",
        )
        .with_marker(ItemMarker::Numbered)
        .with_indent(1),
        fixed_item("00000000-0000-8000-8000-000000004013", ""),
        fixed_item(
            "00000000-0000-8000-8000-000000004012",
            "Before KnotQ, I found myself in the paradoxical state of having so many things I needed to get done yet not doing anything because everything was such a large task. Breaking these into actionable tasks daily helped me a lot.",
        )
        .with_indent(1),
    ]
}

fn fixed_scheme_id(id: &str) -> SchemeId {
    id.parse().expect("valid fixed starter scheme id")
}

fn fixed_document_id(id: &str) -> DocumentId {
    id.parse().expect("valid fixed starter document id")
}

fn fixed_item_id(id: &str) -> ItemId {
    id.parse().expect("valid fixed starter item id")
}

fn fixed_item(id: &str, text: &str) -> Item {
    let mut item = Item::new(text);
    item.id = fixed_item_id(id);
    item
}

fn fixed_scheme_sync(document_id: DocumentId) -> SyncDocumentMeta {
    let mut meta = SyncDocumentMeta::local(SyncDocumentKind::Scheme);
    meta.id = document_id;
    meta
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
    item.text().trim().is_empty()
        && !item.has_images()
        && !item.has_table()
        && item.marker == ItemMarker::Blank
        && item.indent == 0
        && !daily_queue_item_has_annotations(item)
        && item.priority.is_none()
        && item.state.len() == 1
        && item.state[0].state.progress == 0
        && item.state[0].state.notification_offset_secs.is_none()
}
