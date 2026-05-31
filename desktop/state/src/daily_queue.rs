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
    if daily_queue_scheme_is_blank(today) {
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

pub fn daily_queue_scheme_is_blank(scheme: &Scheme) -> bool {
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
    insert_root_scheme(&mut workspace, make_homework_scheme(today));
    insert_root_scheme(&mut workspace, make_classes_scheme(today));
    insert_root_scheme(&mut workspace, make_life_scheme(today));

    let yesterday = today - Duration::days(1);
    let mut past_daily = Scheme::new(
        daily_queue_scheme_name(yesterday),
        knotq_model::DAILY_QUEUE_COLOR_INDEX,
    );
    past_daily.id = knotq_model::daily_queue_scheme_id(yesterday);
    past_daily.items = vec![Item::new("Past days show up here")];
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

fn make_homework_scheme(today: NaiveDate) -> Scheme {
    let mut scheme = Scheme::new("Homework", 5);
    scheme.items = vec![
        Item::new("Finish problem set").with_end(local_dt(today + Duration::days(1), 23, 0)),
        Item::new("Try using ctrl/cmd r to make assignments repeating")
            .with_marker(ItemMarker::Bullet),
    ];
    scheme
}

fn make_classes_scheme(today: NaiveDate) -> Scheme {
    let mut scheme = Scheme::new("Classes", 0);
    scheme.items = vec![
        Item::new("Calculus lecture")
            .with_start(local_dt(today, 13, 0))
            .with_end(local_dt(today, 14, 15))
            .done(),
        Item::new(""),
    ];
    scheme
}

fn make_life_scheme(today: NaiveDate) -> Scheme {
    let mut scheme = Scheme::new("Life", 3);
    scheme.items = vec![
        Item::new("Laundry").with_start(local_dt(today + Duration::days(1), 18, 30)),
        Item::new(""),
    ];
    scheme
}

fn make_daily_seed_items() -> Vec<Item> {
    vec![
        Item::new("Daily is where you put quick notes, brainstorm, and list tasks you want to complete for the day e.g."),
        Item::new("Edit paper").done(),
        Item::new("Film video").with_marker(ItemMarker::Checkbox),
        Item::new(""),
        Item::new("You can put calendar items here or in the main items and they come in three types"),
        Item::new("Reminders only have a start (try cmd/ctrl s)").with_marker(ItemMarker::Numbered),
        Item::new("or click on calendar")
            .with_marker(ItemMarker::Bullet)
            .with_indent(1),
        Item::new("Assignments only have an end (try cmd/ctrl e)").with_marker(ItemMarker::Numbered),
        Item::new("or shift click on calendar")
            .with_marker(ItemMarker::Bullet)
            .with_indent(1),
        Item::new("Events have both a start and end (try doing both)")
            .with_marker(ItemMarker::Numbered),
        Item::new("or drag on calendar")
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
