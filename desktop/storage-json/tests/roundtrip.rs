use chrono::{NaiveDate, TimeZone, Utc};
use knotq_model::{
    AppSettings, CalendarProvider, CalendarWeekRange, ExternalItemSource, Folder, FolderId,
    GoogleOAuthAccount, ImageAssetFormat, Item, ItemMarker, ItemMedia, NodeRef, Scheme, ThemeMode,
    Workspace,
};
use knotq_storage_json::{
    load_app_settings, load_daily_queue_scheme, load_daily_queue_schemes_for_calendar_range,
    load_workspace, load_workspace_with_options, restore_workspace_snapshot, save_app_settings,
    save_workspace, scheme_path_for_workspace, WorkspaceLoadOptions,
};
use std::{fs, path::PathBuf};

#[test]
fn load_app_settings_rejects_raw_settings_payload() {
    let dir = unique_temp_dir("knotq-settings-raw");
    let settings_file = dir.join("settings.json");
    let raw = serde_json::to_string_pretty(&knotq_model::AppSettings::default()).unwrap();
    fs::write(&settings_file, raw).unwrap();

    let err = load_app_settings(&settings_file).unwrap_err();
    assert!(err.to_string().contains("parse settings.json"));

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn app_settings_roundtrip_preserves_google_accounts() {
    let dir = unique_temp_dir("knotq-settings-google-account");
    let settings_file = dir.join("settings.json");
    let mut settings = AppSettings::default();
    settings.calendar_week_range = CalendarWeekRange::CalendarWeek;
    settings.google_accounts.push(GoogleOAuthAccount {
        account_id: "sub-1".to_string(),
        email: Some("user@example.com".to_string()),
        client_id: "client.apps.googleusercontent.com".to_string(),
        access_token: "access".to_string(),
        refresh_token: "refresh".to_string(),
        expires_at: Some(Utc.with_ymd_and_hms(2026, 5, 18, 12, 0, 0).unwrap()),
        scope: "https://www.googleapis.com/auth/calendar.events".to_string(),
    });

    save_app_settings(&settings_file, &settings).unwrap();
    let loaded = load_app_settings(&settings_file).unwrap();

    assert_eq!(loaded.google_accounts.len(), 1);
    assert_eq!(
        loaded.google_accounts[0].email.as_deref(),
        Some("user@example.com")
    );
    assert_eq!(loaded.calendar_week_range, CalendarWeekRange::CalendarWeek);

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn save_workspace_splits_scheme_files_and_omits_empty_item_fields() {
    let dir = unique_temp_dir("knotq-storage-split");
    let workspace_file = dir.join("workspace.json");
    let mut workspace = Workspace::new();
    let root = workspace.root;
    let mut scheme = Scheme::new("Notes", 2);
    scheme.items.push(Item::new("plain"));
    let mut done = Item::new("done");
    done.marker = ItemMarker::Checkbox;
    done.state[0].state.progress = -1;
    scheme.items.push(done);
    let mut image_item = Item::new("image");
    let image_item_id = image_item.id;
    image_item.external = Some(ExternalItemSource {
        provider: CalendarProvider::Google,
        account_id: "google".to_string(),
        calendar_id: "work".to_string(),
        event_id: "event-1".to_string(),
        instance_id: None,
        updated_at: Some(Utc.with_ymd_and_hms(2026, 5, 18, 12, 0, 0).unwrap()),
    });
    image_item.media.push(ItemMedia::Image {
        asset: uuid::Uuid::new_v4(),
        format: ImageAssetFormat::Png,
        width: Some(320),
        height: Some(180),
    });
    scheme.items.push(image_item);
    let scheme_id = scheme.id;
    workspace.schemes.insert(scheme_id, scheme);
    workspace
        .folders
        .get_mut(&root)
        .unwrap()
        .children
        .push(NodeRef::Scheme(scheme_id));

    save_workspace(&workspace_file, &workspace).unwrap();

    let index = fs::read_to_string(&workspace_file).unwrap();
    assert!(index.contains("\"version\": 1"));
    assert!(!index.contains("\"items\""));
    assert!(fs::read_to_string(dir.join(".gitignore"))
        .unwrap()
        .contains("backups/"));

    let scheme_path = scheme_path_for_workspace(&dir, &workspace, scheme_id)
        .unwrap()
        .unwrap();
    assert_eq!(scheme_path, dir.join("schemes").join("Notes.knotq"));
    let scheme_markdown = fs::read_to_string(scheme_path).unwrap();
    assert!(scheme_markdown.starts_with("plain\n- [x] done\nimage "));
    assert!(!scheme_markdown.starts_with("!knotq{type=\"scheme\""));
    assert!(!scheme_markdown.contains("\"items\""));
    assert!(!scheme_markdown.contains("start="));
    assert!(!scheme_markdown.contains("available="));
    assert!(!scheme_markdown.contains("priority="));
    assert!(scheme_markdown.contains("media="));
    assert!(scheme_markdown.contains("external="));
    assert!(scheme_markdown.contains("google"));
    assert!(scheme_markdown.contains("png"));
    assert_eq!(scheme_markdown.matches("id=").count(), 1);
    assert!(scheme_markdown.contains(&format!("id=\"{image_item_id}\"")));
    assert!(scheme_markdown.contains("- [x] done"));

    let loaded = load_workspace(&workspace_file).unwrap().unwrap();
    assert_eq!(loaded.schemes[&scheme_id].items.len(), 3);
    assert_eq!(loaded.schemes[&scheme_id].items[0].text, "plain");
    assert_eq!(
        loaded.schemes[&scheme_id].items[2]
            .external
            .as_ref()
            .unwrap()
            .event_id,
        "event-1"
    );
    assert_eq!(loaded.schemes[&scheme_id].items[2].id, image_item_id);

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn schemes_are_saved_under_named_folder_paths() {
    let dir = unique_temp_dir("knotq-storage-named-paths");
    let workspace_file = dir.join("workspace.json");
    let mut workspace = Workspace::new();
    let root = workspace.root;
    let folder_id = FolderId::new();
    let mut scheme = Scheme::new("Research", 4);
    scheme.items.push(Item::new("Read paper"));
    let scheme_id = scheme.id;

    workspace.schemes.insert(scheme_id, scheme);
    workspace.folders.insert(
        folder_id,
        Folder {
            id: folder_id,
            name: "Projects".into(),
            parent: Some(root),
            children: vec![NodeRef::Scheme(scheme_id)],
            expanded: true,
        },
    );
    workspace
        .folders
        .get_mut(&root)
        .unwrap()
        .children
        .push(NodeRef::Folder(folder_id));

    save_workspace(&workspace_file, &workspace).unwrap();

    let path = dir.join("schemes").join("Projects").join("Research.knotq");
    assert!(path.exists());
    assert!(fs::read_to_string(&path).unwrap().contains("Read paper"));

    let loaded = load_workspace(&workspace_file).unwrap().unwrap();
    assert_eq!(loaded.schemes[&scheme_id].items[0].text, "Read paper");

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn schemes_are_saved_under_nested_folder_paths() {
    let dir = unique_temp_dir("knotq-storage-nested-paths");
    let workspace_file = dir.join("workspace.json");
    let mut workspace = Workspace::new();
    let root = workspace.root;
    let projects_id = FolderId::new();
    let research_id = FolderId::new();
    let mut scheme = Scheme::new("ELSAN", 4);
    scheme.items.push(Item::new("Read paper"));
    let scheme_id = scheme.id;

    workspace.schemes.insert(scheme_id, scheme);
    workspace.folders.insert(
        projects_id,
        Folder {
            id: projects_id,
            name: "Projects".into(),
            parent: Some(root),
            children: vec![NodeRef::Folder(research_id)],
            expanded: true,
        },
    );
    workspace.folders.insert(
        research_id,
        Folder {
            id: research_id,
            name: "Research".into(),
            parent: Some(projects_id),
            children: vec![NodeRef::Scheme(scheme_id)],
            expanded: true,
        },
    );
    workspace
        .folders
        .get_mut(&root)
        .unwrap()
        .children
        .push(NodeRef::Folder(projects_id));

    save_workspace(&workspace_file, &workspace).unwrap();

    let path = dir
        .join("schemes")
        .join("Projects")
        .join("Research")
        .join("ELSAN.knotq");
    assert!(path.exists());

    let loaded = load_workspace(&workspace_file).unwrap().unwrap();
    assert_eq!(loaded.schemes[&scheme_id].items[0].text, "Read paper");

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn missing_daily_queue_files_do_not_block_workspace_load() {
    let dir = unique_temp_dir("knotq-storage-missing-daily");
    let workspace_file = dir.join("workspace.json");
    let date = NaiveDate::from_ymd_opt(2026, 5, 17).unwrap();
    let mut workspace = Workspace::new();

    let mut daily = Scheme::new("Daily 2026-05-17", 0);
    daily.items.push(Item::new("old daily note"));
    let daily_id = daily.id;
    workspace.daily_queue.insert(date, daily_id);
    workspace.schemes.insert(daily_id, daily);

    save_workspace(&workspace_file, &workspace).unwrap();
    fs::remove_file(
        dir.join("daily_queue")
            .join("2026")
            .join("05")
            .join("17.knotq"),
    )
    .unwrap();

    let loaded = load_workspace_with_options(
        &workspace_file,
        WorkspaceLoadOptions::daily_queue_range(date, date),
    )
    .unwrap()
    .unwrap();
    assert_eq!(loaded.daily_queue_scheme_id(date), Some(daily_id));
    assert!(!loaded.schemes.contains_key(&daily_id));

    save_workspace(&workspace_file, &loaded).unwrap();
    assert!(fs::read_to_string(&workspace_file)
        .unwrap()
        .contains("Daily 2026-05-17"));

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn save_workspace_rejects_duplicate_scheme_file_paths() {
    let dir = unique_temp_dir("knotq-storage-duplicate-paths");
    let workspace_file = dir.join("workspace.json");
    let mut workspace = Workspace::new();
    let root = workspace.root;
    let first = Scheme::new("Notes", 0);
    let first_id = first.id;
    let second = Scheme::new("notes", 1);
    let second_id = second.id;

    workspace.schemes.insert(first_id, first);
    workspace.schemes.insert(second_id, second);
    workspace
        .folders
        .get_mut(&root)
        .unwrap()
        .children
        .extend([NodeRef::Scheme(first_id), NodeRef::Scheme(second_id)]);

    let err = save_workspace(&workspace_file, &workspace).unwrap_err();
    assert!(err
        .to_string()
        .contains("multiple schemes resolve to the same file"));

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn workspace_load_options_keep_unrequested_daily_queue_index_only() {
    let dir = unique_temp_dir("knotq-storage-daily-lazy-load");
    let workspace_file = dir.join("workspace.json");
    let old_date = NaiveDate::from_ymd_opt(2026, 4, 1).unwrap();
    let today = NaiveDate::from_ymd_opt(2026, 5, 17).unwrap();
    let mut workspace = Workspace::new();

    let mut old = Scheme::new("Daily 2026-04-01", 0);
    old.items.push(Item::new("old note"));
    let old_id = old.id;
    workspace.daily_queue.insert(old_date, old_id);
    workspace.schemes.insert(old_id, old);

    let mut current = Scheme::new("Daily 2026-05-17", 0);
    current.items.push(Item::new("today note"));
    let current_id = current.id;
    workspace.daily_queue.insert(today, current_id);
    workspace.schemes.insert(current_id, current);

    save_workspace(&workspace_file, &workspace).unwrap();

    let loaded = load_workspace_with_options(
        &workspace_file,
        WorkspaceLoadOptions::daily_queue_range(today, today),
    )
    .unwrap()
    .unwrap();

    assert_eq!(loaded.daily_queue_scheme_id(old_date), Some(old_id));
    assert_eq!(loaded.daily_queue_scheme_id(today), Some(current_id));
    assert!(!loaded.schemes.contains_key(&old_id));
    assert_eq!(loaded.schemes[&current_id].items[0].text, "today note");

    let loaded_old = load_daily_queue_scheme(&workspace_file, old_date)
        .unwrap()
        .unwrap();
    assert_eq!(loaded_old.id, old_id);
    assert_eq!(loaded_old.items[0].text, "old note");

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn workspace_load_options_load_daily_queue_by_calendar_index() {
    let dir = unique_temp_dir("knotq-storage-daily-calendar-index");
    let workspace_file = dir.join("workspace.json");
    let old_date = NaiveDate::from_ymd_opt(2026, 3, 10).unwrap();
    let today = NaiveDate::from_ymd_opt(2026, 5, 17).unwrap();
    let reminder_time = Utc.with_ymd_and_hms(2026, 5, 17, 14, 30, 0).unwrap();
    let mut workspace = Workspace::new();

    let mut old = Scheme::new("Daily 2026-03-10", 0);
    let mut reminder = Item::new("shows today");
    reminder.marker = ItemMarker::Checkbox;
    reminder.start = Some(reminder_time);
    old.items.push(reminder);
    let old_id = old.id;
    workspace.daily_queue.insert(old_date, old_id);
    workspace.schemes.insert(old_id, old);

    let mut current = Scheme::new("Daily 2026-05-17", 0);
    current.items.push(Item::new("today note"));
    let current_id = current.id;
    workspace.daily_queue.insert(today, current_id);
    workspace.schemes.insert(current_id, current);

    save_workspace(&workspace_file, &workspace).unwrap();

    let loaded = load_workspace_with_options(
        &workspace_file,
        WorkspaceLoadOptions::daily_queue_range(today, today),
    )
    .unwrap()
    .unwrap();

    assert_eq!(loaded.schemes[&old_id].items[0].text, "shows today");
    assert_eq!(loaded.schemes[&current_id].items[0].text, "today note");

    let loaded_for_range =
        load_daily_queue_schemes_for_calendar_range(&workspace_file, today, today).unwrap();
    assert_eq!(loaded_for_range.len(), 1);
    assert_eq!(loaded_for_range[0].0, old_date);
    assert_eq!(loaded_for_range[0].1.id, old_id);

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn version_history_restores_saved_workspace_files() {
    let dir = unique_temp_dir("knotq-storage-history-restore");
    let workspace_file = dir.join("workspace.json");
    let mut workspace = Workspace::new();
    let root = workspace.root;
    let folder_id = FolderId::new();
    let mut scheme = Scheme::new("Draft", 3);
    scheme.items.push(Item::new("first version"));
    let scheme_id = scheme.id;

    workspace.schemes.insert(scheme_id, scheme);
    workspace.folders.insert(
        folder_id,
        Folder {
            id: folder_id,
            name: "Projects".into(),
            parent: Some(root),
            children: vec![NodeRef::Scheme(scheme_id)],
            expanded: true,
        },
    );
    workspace
        .folders
        .get_mut(&root)
        .unwrap()
        .children
        .push(NodeRef::Folder(folder_id));

    save_workspace(&workspace_file, &workspace).unwrap();
    let snapshots = knotq_storage_json::list_workspace_snapshots(&dir).unwrap();
    assert_eq!(snapshots.len(), 1);
    let first_snapshot = snapshots[0].id.clone();
    let first_scheme_path = dir.join("schemes").join("Projects").join("Draft.knotq");
    assert!(first_scheme_path.exists());

    workspace.schemes.get_mut(&scheme_id).unwrap().items[0].text = "second version".into();
    save_workspace(&workspace_file, &workspace).unwrap();
    assert!(fs::read_to_string(&first_scheme_path)
        .unwrap()
        .contains("second version"));

    restore_workspace_snapshot(&dir, &first_snapshot).unwrap();
    let restored = load_workspace(&workspace_file).unwrap().unwrap();

    assert_eq!(restored.schemes[&scheme_id].items[0].text, "first version");
    assert!(fs::read_to_string(&first_scheme_path)
        .unwrap()
        .contains("first version"));

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn app_settings_default_to_dark_theme() {
    assert_eq!(
        knotq_model::AppSettings::default().theme_mode,
        ThemeMode::Dark
    );
    assert_eq!(
        knotq_model::AppSettings::default().calendar_week_range,
        CalendarWeekRange::NextSevenDays
    );
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "{prefix}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&path).unwrap();
    path
}
