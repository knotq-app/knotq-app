use chrono::{Datelike, Duration, Local, LocalResult, NaiveDate, TimeZone, Utc};
use knotq_model::{
    AppSettings, CalendarViewMode, Item, ItemMarker, NodeRef, SavedWindowPosition, SavedWindowSize,
    Scheme, ThemeMode, Workspace,
};
use knotq_storage_json::{save_app_settings, save_workspace};
use std::{env, path::PathBuf};

fn main() -> anyhow::Result<()> {
    let Some(data_dir) = env::args_os().nth(1).map(PathBuf::from) else {
        anyhow::bail!("usage: website_seed <data-dir>");
    };

    std::fs::create_dir_all(&data_dir)?;
    let workspace_dir = data_dir.join("workspace");
    std::fs::create_dir_all(&workspace_dir)?;

    let settings = AppSettings {
        theme_mode: ThemeMode::Dark,
        calendar_view: CalendarViewMode::Week,
        onboarding_completed: true,
        window_size: Some(SavedWindowSize {
            width: 1360.0,
            height: 860.0,
        }),
        window_position: Some(SavedWindowPosition { x: 80.0, y: 80.0 }),
        ..Default::default()
    };
    save_app_settings(&data_dir.join("settings.json"), &settings)?;

    let today = Local::now().date_naive();
    let tomorrow = today + Duration::days(1);
    let next = today + Duration::days(2);
    let plus3 = today + Duration::days(3);

    let mut workspace = Workspace::new();
    let root = workspace.root;

    let mut roadmap = Scheme::new("Product Roadmap", 3);
    roadmap.items = vec![
        heading("# Quarterly Product Roadmap"),
        plain("Planning doc for Q3 launch milestones"),
        plain(""),
        heading("## Schedule from individual lines"),
        plain("Every scheduled entry below is just a line in this document:"),
        event("Prototype review with design", today, 10, 0, today, 10, 45),
        assignment("Send revised launch brief", today, 17, 0),
        reminder("Ping Sam about onboarding copy", tomorrow, 9, 0),
        plain(""),
        heading("## Current design"),
        checkbox("Finalize dashboard wireframe").done(),
        checkbox("Review searchable timeline behavior").done(),
        checkbox("Build onboarding flow prototype"),
        child("Show schedule controls on the exact line being edited"),
        checkbox("Draft calendar handoff notes"),
        plain(""),
        heading("## Launch beta"),
        bullet("Sprint 1: line scheduling polish"),
        bullet("Sprint 2: editor performance pass"),
        bullet("Sprint 3: calendar import QA"),
        event("Product walkthrough", tomorrow, 13, 0, tomorrow, 14, 0),
        assignment("Tag beta release", plus3, 14, 30),
    ];
    let roadmap_id = add_root_scheme(&mut workspace, root, roadmap);

    let mut meetings = Scheme::new("Meeting Notes", 2);
    meetings.items = vec![
        heading("# Meeting Notes"),
        event("Roadmap review", today, 11, 30, today, 12, 0),
        checkbox("Capture decisions from design review"),
        assignment("Send weekly status update", next, 17, 0),
    ];
    let meetings_id = add_root_scheme(&mut workspace, root, meetings);

    let mut personal = Scheme::new("Personal", 5);
    personal.items = vec![
        reminder("Pick up dry cleaning", tomorrow, 16, 30),
        event("Dinner with Sam", next, 18, 30, next, 20, 0),
        checkbox("Book train ticket"),
    ];
    let personal_id = add_root_scheme(&mut workspace, root, personal);

    let mut reading = Scheme::new("Reading List", 1);
    reading.items = vec![
        checkbox("Line-first planning notes"),
        checkbox("Calendar UX references"),
        assignment("Read scheduling API doc", tomorrow, 11, 0),
    ];
    let reading_id = add_root_scheme(&mut workspace, root, reading);

    let mut ideas = Scheme::new("Ideas", 4);
    ideas.items = vec![
        bullet("Inline schedule preview"),
        bullet("Drag a document line into the week view"),
        reminder("Sketch launch card concepts", plus3, 9, 30),
    ];
    let ideas_id = add_root_scheme(&mut workspace, root, ideas);

    let mut work = Scheme::new("Work", 0);
    work.items = vec![
        assignment("Performance reviews", today, 17, 0),
        event("Customer interview", tomorrow, 15, 0, tomorrow, 15, 45),
        checkbox("Prep sprint board"),
    ];
    let work_id = add_root_scheme(&mut workspace, root, work);

    let mut daily = Scheme::new("Daily", knotq_model::DAILY_QUEUE_COLOR_INDEX);
    daily.items = vec![
        checkbox("Make the website answer: why KnotQ?"),
        child("Each scheduled item still starts as a document line"),
        checkbox("Take product screenshots from a seeded workspace"),
        checkbox("Tighten the download call to action"),
        checkbox("Write release copy"),
    ];
    let daily_id = daily.id;
    workspace.daily_queue.insert(today, daily_id);
    workspace.schemes.insert(daily_id, daily);

    let _ = (
        roadmap_id,
        meetings_id,
        personal_id,
        reading_id,
        ideas_id,
        work_id,
    );
    save_workspace(&workspace_dir.join("workspace.json"), &workspace)?;
    Ok(())
}

fn add_root_scheme(
    workspace: &mut Workspace,
    root: knotq_model::FolderId,
    scheme: Scheme,
) -> knotq_model::SchemeId {
    let id = scheme.id;
    workspace.schemes.insert(id, scheme);
    workspace
        .folders
        .get_mut(&root)
        .unwrap()
        .children
        .push(NodeRef::Scheme(id));
    id
}

fn heading(text: &str) -> Item {
    Item::new(text)
}

fn plain(text: &str) -> Item {
    Item::new(text)
}

fn checkbox(text: &str) -> Item {
    Item::new(text).with_marker(ItemMarker::Checkbox)
}

fn bullet(text: &str) -> Item {
    Item::new(text).with_marker(ItemMarker::Bullet)
}

fn child(text: &str) -> Item {
    Item::new(text).with_indent(1)
}

fn event(
    text: &str,
    start_date: NaiveDate,
    start_hour: u32,
    start_minute: u32,
    end_date: NaiveDate,
    end_hour: u32,
    end_minute: u32,
) -> Item {
    Item::new(text)
        .with_start(local_utc(start_date, start_hour, start_minute))
        .with_end(local_utc(end_date, end_hour, end_minute))
}

fn assignment(text: &str, date: NaiveDate, hour: u32, minute: u32) -> Item {
    Item::new(text).with_end(local_utc(date, hour, minute))
}

fn reminder(text: &str, date: NaiveDate, hour: u32, minute: u32) -> Item {
    Item::new(text).with_start(local_utc(date, hour, minute))
}

fn local_utc(date: NaiveDate, hour: u32, minute: u32) -> chrono::DateTime<Utc> {
    match Local.with_ymd_and_hms(date.year(), date.month(), date.day(), hour, minute, 0) {
        LocalResult::Single(dt) => dt.with_timezone(&Utc),
        LocalResult::Ambiguous(dt, _) => dt.with_timezone(&Utc),
        LocalResult::None => Utc
            .with_ymd_and_hms(date.year(), date.month(), date.day(), hour, minute, 0)
            .unwrap(),
    }
}
