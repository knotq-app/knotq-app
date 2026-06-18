use chrono::{Datelike, Duration, Local, LocalResult, NaiveDate, TimeZone, Utc};
use knotq_model::{
    daily_queue_scheme_id, AppSettings, CalendarViewMode, Item, ItemMarker, NodeRef,
    SavedView, SavedWindowPosition, SavedWindowSize, Scheme, ThemeMode, Workspace,
    DAILY_QUEUE_COLOR_INDEX,
};
use knotq_storage_json::{save_app_settings, save_workspace};
use std::{env, path::PathBuf};

/// Seeds a throwaway data directory used to retake the knotq.com screenshots.
/// It opens on the Daily Queue in the Light theme and reproduces the layout in
/// `website/img/knotq-daily.webp`: a small set of color-coded schemes feeding the
/// upcoming panel, plus two days of daily-queue notes (yesterday + today).
///
/// Usage: `website_seed <data-dir>` where `<data-dir>` is the KnotQ data dir, e.g.
/// `"$HOME/Library/Application Support/KnotQ"` for whatever HOME the app launches with.
fn main() -> anyhow::Result<()> {
    let Some(data_dir) = env::args_os().nth(1).map(PathBuf::from) else {
        anyhow::bail!("usage: website_seed <data-dir>");
    };

    std::fs::create_dir_all(&data_dir)?;
    let workspace_dir = data_dir.join("workspace");
    std::fs::create_dir_all(&workspace_dir)?;

    let settings = AppSettings {
        theme_mode: ThemeMode::Light,
        calendar_view: CalendarViewMode::Week,
        onboarding_completed: true,
        // Open straight onto the Daily Queue for the screenshot.
        last_view: Some(SavedView::DailyQueue),
        window_size: Some(SavedWindowSize {
            width: 1360.0,
            height: 860.0,
        }),
        window_position: Some(SavedWindowPosition { x: 80.0, y: 80.0 }),
        ..Default::default()
    };
    save_app_settings(&data_dir.join("settings.json"), &settings)?;

    let today = Local::now().date_naive();
    let yesterday = today - Duration::days(1);
    let tomorrow = today + Duration::days(1);

    let mut workspace = Workspace::new();
    let root = workspace.root;

    // ── Sidebar schemes (order + color matches the screenshot) ───────────────
    // Colors: 0 red, 1 orange, 2 green, 3 blue, 5 amber.

    let mut work = Scheme::new("Work", 0);
    work.items = vec![
        heading("# Work"),
        assignment("Performance reviews due", today + Duration::days(5), 17, 0),
        checkbox("Prep sprint board"),
        checkbox("Triage incoming bugs"),
    ];
    add_root_scheme(&mut workspace, root, work);

    let mut roadmap = Scheme::new("Product Roadmap", 3);
    roadmap.items = vec![
        heading("# Product Roadmap"),
        plain("Planning doc for the beta launch milestones"),
        assignment("Beta launch prep doc", today + Duration::days(12), 23, 0),
        reminder("Send weekly status update", today + Duration::days(3), 17, 0),
        plain(""),
        heading("## Launch checklist"),
        checkbox("Finalize onboarding flow"),
        checkbox("Editor performance pass"),
        checkbox("Calendar import QA"),
    ];
    add_root_scheme(&mut workspace, root, roadmap);

    let mut personal = Scheme::new("Personal", 2);
    personal.items = vec![
        heading("# Personal"),
        reminder("Pick up dry cleaning", tomorrow, 16, 0),
        checkbox("Book train ticket"),
    ];
    add_root_scheme(&mut workspace, root, personal);

    let mut reading = Scheme::new("Reading List", 1);
    reading.items = vec![
        heading("# Reading List"),
        checkbox("Line-first planning notes"),
        checkbox("Calendar UX references"),
        bullet("Local-first sync write-ups"),
    ];
    add_root_scheme(&mut workspace, root, reading);

    let mut ideas = Scheme::new("Ideas", 5);
    ideas.items = vec![
        heading("# Ideas"),
        bullet("Inline schedule preview"),
        bullet("Drag a document line into the week view"),
        bullet("Sketch launch card concepts"),
    ];
    add_root_scheme(&mut workspace, root, ideas);

    // ── Daily queue: yesterday (carry-over work) + today ─────────────────────

    add_daily_queue(
        &mut workspace,
        yesterday,
        vec![
            checkbox("Fix search regression").done(),
            child("Was the unindexed tags query"),
            checkbox("Draft sprint retro notes").done(),
            checkbox("Prep design review slides"),
            child("Include wireframes from Sarah"),
            child("Add competitive analysis screenshots"),
            bullet("Idea: weekly email digest for beta users"),
        ],
    );

    add_daily_queue(
        &mut workspace,
        today,
        vec![
            checkbox("Finish self-review for perf cycle"),
            checkbox("Reply to Sarah's wireframe feedback"),
            checkbox("Book flights for June trip"),
            checkbox("Grocery run after work"),
            child("Olive oil, bread"),
            bullet("Morning thought: should we add markdown export?"),
            child("Would make sharing schemes much easier"),
            child("Could reuse the .knotq parser"),
        ],
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

/// Insert one day's daily-queue scheme, using the stable per-date id the app
/// expects so it loads and renders the entry on the Daily Queue.
fn add_daily_queue(workspace: &mut Workspace, date: NaiveDate, items: Vec<Item>) {
    let id = daily_queue_scheme_id(date);
    let mut scheme = Scheme::new("Daily", DAILY_QUEUE_COLOR_INDEX);
    scheme.id = id;
    scheme.items = items;
    workspace.schemes.insert(id, scheme);
    workspace.daily_queue.insert(date, id);
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
