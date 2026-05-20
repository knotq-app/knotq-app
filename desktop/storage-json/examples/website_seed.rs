use chrono::{Datelike, Duration, Local, LocalResult, NaiveDate, TimeZone, Utc};
use knotq_model::{
    AppSettings, CalendarViewMode, Folder, FolderId, Item, ItemMarker, NodeRef,
    SavedWindowPosition, SavedWindowSize, Scheme, ThemeMode, Workspace,
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

    let mut settings = AppSettings::default();
    settings.theme_mode = ThemeMode::Light;
    settings.calendar_view = CalendarViewMode::Week;
    settings.onboarding_completed = true;
    settings.window_size = Some(SavedWindowSize {
        width: 1360.0,
        height: 860.0,
    });
    settings.window_position = Some(SavedWindowPosition { x: 80.0, y: 80.0 });
    save_app_settings(&data_dir.join("settings.json"), &settings)?;

    let today = Local::now().date_naive();
    let yesterday = today - Duration::days(1);
    let tomorrow = today + Duration::days(1);
    let next = today + Duration::days(2);
    let plus3 = today + Duration::days(3);
    let plus4 = today + Duration::days(4);

    let mut workspace = Workspace::new();
    let root = workspace.root;

    let launch_folder = add_folder(&mut workspace, root, "Launch");
    let planning_folder = add_folder(&mut workspace, root, "Planning");
    let life_folder = add_folder(&mut workspace, root, "Life");

    let mut launch = Scheme::new("KnotQ Launch", 1);
    launch.items = vec![
        checkbox("Ship landing page with real screenshots"),
        child("Hero: one sentence for why documents and calendars belong together"),
        event("Launch planning", yesterday, 9, 30, yesterday, 10, 15),
        event(
            "App walkthrough capture",
            yesterday,
            14,
            0,
            yesterday,
            15,
            0,
        ),
        event("Design review", today, 9, 0, today, 10, 0),
        event(
            "Pricing and positioning review",
            today,
            10,
            30,
            today,
            11,
            15,
        ),
        child("Decide whether to lead with Daily, Calendar, or Schemes"),
        event("Beta install debugging", today, 11, 45, today, 12, 30),
        event("Release checklist pass", today, 13, 30, today, 14, 15),
        assignment("Publish beta notes", today, 16, 0),
        reminder("Follow up with beta users", tomorrow, 9, 0),
        event("Website copy pass", tomorrow, 10, 15, tomorrow, 11, 0),
        event("Release notes edit", plus3, 10, 0, plus3, 11, 0),
        assignment("Cut onboarding demo", tomorrow, 17, 30),
        assignment("Tag beta release", plus3, 14, 30),
        checkbox("Record 45-second walkthrough clip"),
    ];
    let launch_id = add_scheme(&mut workspace, launch_folder, launch);

    let mut week = Scheme::new("Week Plan", 4);
    week.items = vec![
        bullet("Monday"),
        event("Weekly review", yesterday, 8, 30, yesterday, 9, 15).with_indent(1),
        checkbox("Move loose notes into launch plan").with_indent(1),
        bullet("Tuesday"),
        event("Customer interview", today, 14, 0, today, 14, 45).with_indent(1),
        event("Calendar density review", today, 15, 15, today, 16, 0).with_indent(1),
        bullet("Wednesday"),
        event("Product walkthrough", tomorrow, 13, 0, tomorrow, 14, 30).with_indent(1),
        event("Implementation block", next, 9, 0, next, 11, 30).with_indent(1),
        event("Open source cleanup", next, 12, 30, next, 13, 30).with_indent(1),
        assignment("Send weekly update", next, 15, 30).with_indent(1),
        bullet("Friday"),
        event("Website QA sweep", plus3, 9, 0, plus3, 10, 0).with_indent(1),
        event("Beta feedback triage", plus3, 13, 0, plus3, 14, 0).with_indent(1),
    ];
    let week_id = add_scheme(&mut workspace, planning_folder, week);

    let mut research = Scheme::new("Research Notes", 2);
    research.items = vec![
        bullet("Why most task apps split attention"),
        child("Tasks live in one place, calendar commitments live somewhere else"),
        child("A line should be able to carry context and time at once"),
        checkbox("Compare agenda flows in Things, Notion, and Calendar"),
        event("Competitor notes sweep", today, 12, 45, today, 13, 30),
        event("Positioning research", tomorrow, 15, 0, tomorrow, 16, 0),
        reminder("Capture onboarding feedback", next, 10, 0),
        assignment("Summarize interview patterns", next, 18, 0),
        event("Essay on line-first planning", plus4, 11, 0, plus4, 12, 30),
    ];
    let research_id = add_scheme(&mut workspace, planning_folder, research);

    let mut life = Scheme::new("Personal", 3);
    life.items = vec![
        reminder("Pack returns", today, 8, 30),
        event("Dinner with Sam", tomorrow, 18, 30, tomorrow, 20, 0),
        event("Gym", next, 7, 30, next, 8, 30),
        event("Errands loop", plus3, 16, 0, plus3, 17, 0),
        event("Coffee with Mira", plus4, 9, 30, plus4, 10, 30),
        assignment("Renew domain", next, 12, 0),
        checkbox("Laundry"),
        child("Move to tonight if the afternoon slips"),
    ];
    let life_id = add_scheme(&mut workspace, life_folder, life);

    let mut daily = Scheme::new("Daily", knotq_model::DAILY_QUEUE_COLOR_INDEX);
    daily.items = vec![
        checkbox("Make the website answer: why KnotQ?"),
        child("A document is the source of truth; the calendar is a view"),
        checkbox("Take product screenshots from a seeded workspace"),
        checkbox("Reply to beta feedback without duplicating calendar events"),
        checkbox("Tighten the download call to action"),
        child("Keep the calendar dense, but make each block readable"),
        checkbox("Write release copy"),
    ];
    let daily_id = daily.id;
    workspace.daily_queue.insert(today, daily_id);
    workspace.schemes.insert(daily_id, daily);

    workspace.folders.get_mut(&root).unwrap().children.extend([
        NodeRef::Folder(launch_folder),
        NodeRef::Folder(planning_folder),
        NodeRef::Folder(life_folder),
    ]);

    let _ = (launch_id, week_id, research_id, life_id);
    save_workspace(&workspace_dir.join("workspace.json"), &workspace)?;
    Ok(())
}

fn add_folder(workspace: &mut Workspace, parent: FolderId, name: &str) -> FolderId {
    let id = FolderId::new();
    workspace.folders.insert(
        id,
        Folder {
            id,
            name: name.to_string(),
            parent: Some(parent),
            children: Vec::new(),
            expanded: true,
        },
    );
    id
}

fn add_scheme(
    workspace: &mut Workspace,
    folder: FolderId,
    scheme: Scheme,
) -> knotq_model::SchemeId {
    let id = scheme.id;
    workspace.schemes.insert(id, scheme);
    workspace
        .folders
        .get_mut(&folder)
        .unwrap()
        .children
        .push(NodeRef::Scheme(id));
    id
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
