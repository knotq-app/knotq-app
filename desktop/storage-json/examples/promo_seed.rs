use chrono::{Datelike, Duration, Local, LocalResult, NaiveDate, TimeZone, Utc};
use knotq_model::{
    daily_queue_scheme_id, AppSettings, CalendarViewMode, Item, ItemMarker, NodeRef, SavedView,
    SavedWindowPosition, SavedWindowSize, Scheme, ThemeMode, Workspace, DAILY_QUEUE_COLOR_INDEX,
};
use knotq_storage_json::{save_app_settings, save_workspace};
use std::{env, path::PathBuf};

/// Seeds a throwaway data directory used to record the promo video: a full,
/// color-coded week on the calendar, one long-form scheme ("Studio Launch") to
/// type into, and two days of Daily Queue.
///
/// Usage: `promo_seed <data-dir> [dark|light] [calendar|daily] [dentist]`, then launch the
/// app with `KNOTQ_DATA_DIR=<data-dir>`.
fn main() -> anyhow::Result<()> {
    let mut args = env::args_os().skip(1);
    let Some(data_dir) = args.next().map(PathBuf::from) else {
        anyhow::bail!("usage: promo_seed <data-dir> [dark|light] [calendar|daily] [dentist]");
    };
    let light = args.next().is_some_and(|value| value == "light");
    let daily = args.next().is_some_and(|value| value == "daily");
    // Later scenes are recorded separately from the calendar one; this carries
    // over the reminder created by the on-camera click so they still match.
    let with_dentist = args.next().is_some_and(|value| value == "dentist");

    std::fs::create_dir_all(&data_dir)?;
    let workspace_dir = data_dir.join("workspace");
    std::fs::create_dir_all(&workspace_dir)?;

    let settings = AppSettings {
        theme_mode: if light {
            ThemeMode::Light
        } else {
            ThemeMode::Dark
        },
        calendar_view: CalendarViewMode::Week,
        onboarding_completed: true,
        community_prompted: true,
        auto_update: false,
        last_view: Some(if daily {
            SavedView::DailyQueue
        } else {
            SavedView::Union
        }),
        window_size: Some(SavedWindowSize {
            width: 1280.0,
            height: 800.0,
        }),
        window_position: Some(SavedWindowPosition { x: 120.0, y: 60.0 }),
        ..Default::default()
    };
    save_app_settings(&data_dir.join("settings.json"), &settings)?;

    let today = Local::now().date_naive();
    // The calendar is recorded on *next* week so no column is dimmed as past;
    // `day(n)` is the nth day of that week (0 = Monday).
    let next_monday = today + Duration::days(7 - i64::from(today.weekday().num_days_from_monday()));
    let day = |offset: i64| next_monday + Duration::days(offset);
    let soon = |offset: i64| today + Duration::days(offset);

    let mut workspace = Workspace::new();
    let root = workspace.root;

    // Colors: 0 red, 1 orange, 2 green, 3 blue, 4 purple, 5 yellow, 7 cyan.
    // The week is deliberately sparse: two or three short-titled blocks a day,
    // and Friday evening left open for the line typed on camera.

    let mut launch = Scheme::new("Studio Launch", 3);
    launch.items = vec![
        heading("# Studio Launch"),
        plain("Everything for the October release, in one document."),
        plain(""),
        heading("## This week"),
        event("Design review", day(1), (10, 0), (11, 0)),
        event("Record the demo", day(2), (13, 0), (15, 0)),
        assignment("Ship the release", day(3), 17, 0),
        plain(""),
        heading("## Checklist"),
        checkbox("Finalize onboarding flow").done(),
        checkbox("Editor performance pass").done(),
        checkbox("Write the changelog"),
        child_checkbox("Screenshots for the site"),
        child_checkbox("Update the guide"),
        checkbox("Calendar import QA"),
        plain(""),
        heading("## Notes"),
        bullet("Keep the first screen quiet"),
        bullet("Local-first: plain files, no account needed"),
    ]
    .into();
    add_root_scheme(&mut workspace, root, launch);

    let mut work = Scheme::new("Work", 0);
    work.items = vec![
        heading("# Work"),
        event("Deep work", day(0), (10, 0), (12, 0)),
        event("1:1 with Priya", day(1), (14, 0), (14, 45)),
        event("Deep work", day(3), (10, 0), (12, 0)),
        event("Team lunch", day(4), (12, 0), (13, 0)),
        assignment("Quarterly review", soon(1), 18, 0),
        checkbox("Triage incoming bugs"),
    ]
    .into();
    add_root_scheme(&mut workspace, root, work);

    let mut fitness = Scheme::new("Fitness", 2);
    fitness.items = vec![
        heading("# Fitness"),
        event("Morning run", day(0), (7, 0), (7, 45)),
        event("Morning run", day(2), (7, 0), (7, 45)),
        event("Morning run", day(4), (7, 0), (7, 45)),
        event("Climbing", day(1), (18, 0), (19, 30)),
        event("Long ride", day(5), (9, 0), (11, 0)),
        event("Yoga", day(6), (8, 0), (9, 0)),
    ]
    .into();
    add_root_scheme(&mut workspace, root, fitness);

    let mut music = Scheme::new("Music", 4);
    music.items = vec![
        heading("# Music"),
        event("Piano", day(0), (18, 0), (19, 0)),
        event("Piano lesson", day(6), (16, 0), (17, 0)),
        checkbox("Learn the Debussy prelude"),
    ]
    .into();
    add_root_scheme(&mut workspace, root, music);

    let mut home = Scheme::new("Home", 1);
    home.items = vec![
        heading("# Home"),
        event("Brunch", day(5), (12, 0), (13, 30)),
        event("Movie night", day(5), (20, 0), (22, 0)),
        reminder("Call the landlord", soon(1), 12, 30),
        reminder("Water the plants", soon(3), 17, 0),
        event("Dinner with Sam", today, (21, 30), (23, 0)),
    ]
    .into();
    add_root_scheme(&mut workspace, root, home);

    let mut reading = Scheme::new("Reading", 5);
    reading.items = vec![
        heading("# Reading"),
        event("Book club", day(2), (19, 30), (20, 30)),
        event("Reading", day(6), (20, 0), (21, 0)),
        assignment("Finish the essay", soon(2), 21, 0),
        checkbox("A Pattern Language"),
    ]
    .into();
    add_root_scheme(&mut workspace, root, reading);

    let mut travel = Scheme::new("Lisbon Trip", 7);
    travel.items = vec![
        heading("# Lisbon Trip"),
        event("Lisbon call", day(3), (15, 0), (16, 0)),
        assignment("Book the flights", soon(4), 18, 0),
        checkbox("Renew passport").done(),
        checkbox("Find a place in Alfama"),
    ]
    .into();
    add_root_scheme(&mut workspace, root, travel);

    add_daily_queue(
        &mut workspace,
        soon(-1),
        vec![
            checkbox("Fix the search regression").done(),
            child("Was the unindexed tags query"),
            checkbox("Draft sprint retro notes").done(),
            bullet("Maya wants the wireframes by Friday"),
        ],
    );

    // A typical day: tasks, a few notes under them, and loose thoughts.
    add_daily_queue(
        &mut workspace,
        today,
        vec![
            checkbox("Review the onboarding copy").done(),
            checkbox("Profile the editor"),
            child("Compare against last week's numbers"),
            checkbox("Write the changelog"),
            bullet("Standup: sync fix ships Thursday"),
            bullet("Idea: a weekly digest for beta users"),
            child("Could reuse the changelog"),
            checkbox("Groceries after work"),
            child("Olive oil, bread, lemons"),
        ]
        .into_iter()
        .chain(with_dentist.then(|| reminder("Call the dentist", day(3), 14, 0)))
        .collect(),
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
    scheme.items = items.into();
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

fn child_checkbox(text: &str) -> Item {
    checkbox(text).with_indent(1)
}

fn bullet(text: &str) -> Item {
    Item::new(text).with_marker(ItemMarker::Bullet)
}

fn child(text: &str) -> Item {
    Item::new(text).with_indent(1)
}

fn event(text: &str, date: NaiveDate, start: (u32, u32), end: (u32, u32)) -> Item {
    checkbox(text)
        .with_start(local_utc(date, start.0, start.1))
        .with_end(local_utc(date, end.0, end.1))
}

fn assignment(text: &str, date: NaiveDate, hour: u32, minute: u32) -> Item {
    checkbox(text).with_end(local_utc(date, hour, minute))
}

fn reminder(text: &str, date: NaiveDate, hour: u32, minute: u32) -> Item {
    checkbox(text).with_start(local_utc(date, hour, minute))
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
