use chrono::{NaiveDate, TimeZone, Utc};
use knotq_model::{
    AppSettings, CalendarProvider, ExternalItemSource, GoogleOAuthAccount, ImageAssetFormat, Item,
    ItemMarker, ItemMedia, NodeRef, Scheme, ThemeMode, Workspace,
};
use knotq_storage_json::{
    load_app_settings, load_daily_queue_scheme, load_daily_queue_schemes_for_calendar_range,
    load_workspace, load_workspace_with_options, save_app_settings, save_workspace,
    WorkspaceLoadOptions,
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
    let first_item_id = scheme.items[0].id;
    let mut done = Item::new("done");
    done.marker = ItemMarker::Checkbox;
    done.state[0].state.progress = -1;
    scheme.items.push(done);
    let mut image_item = Item::new("image");
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
    assert!(index.contains("\"version\": 3"));
    assert!(!index.contains("\"items\""));

    let scheme_json = fs::read_to_string(scheme_file_path(&dir, scheme_id)).unwrap();
    assert!(scheme_json.contains("\"items\""));
    assert!(!scheme_json.contains("\"start\""));
    assert!(!scheme_json.contains("\"available\""));
    assert!(!scheme_json.contains("\"priority\""));
    assert!(scheme_json.contains("\"media\""));
    assert!(scheme_json.contains("\"external\""));
    assert!(scheme_json.contains("\"provider\": \"google\""));
    assert!(scheme_json.contains("\"format\": \"png\""));
    assert_eq!(scheme_json.matches("\"id\"").count(), 4);
    assert!(scheme_json.contains(&format!("\"id\": \"{first_item_id}\"")));
    assert!(scheme_json.contains("\"progress\": -1"));

    let loaded = load_workspace(&workspace_file).unwrap().unwrap();
    assert_eq!(loaded.schemes[&scheme_id].items.len(), 3);
    assert_eq!(
        loaded.schemes[&scheme_id].items[2]
            .external
            .as_ref()
            .unwrap()
            .event_id,
        "event-1"
    );
    assert_eq!(loaded.schemes[&scheme_id].items[0].id, first_item_id);

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
fn app_settings_default_to_dark_theme() {
    assert_eq!(
        knotq_model::AppSettings::default().theme_mode,
        ThemeMode::Dark
    );
}

fn scheme_file_path(base_dir: &std::path::Path, id: knotq_model::SchemeId) -> PathBuf {
    base_dir.join("schemes").join(format!("{id}.json"))
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
