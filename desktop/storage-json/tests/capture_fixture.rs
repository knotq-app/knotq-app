//! Captures a complete on-disk data directory *as this build writes it*, so a
//! later build can be tested against real released-format bytes rather than a
//! hand-written guess at them.
//!
//! This file is meant to be copied into a *release worktree* and run there —
//! see `tests/fixtures/README.md`. It lives on `main` so the next person does
//! not have to write it again, and so it keeps compiling as the model changes.
//!
//! Run with `KNOTQ_FIXTURE_OUT=<dir>`; does nothing otherwise.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use chrono::{Duration, NaiveDate, TimeZone, Utc};
use knotq_model::*;
use knotq_storage_json::{crdt_state_path, save_app_settings, save_crdt_state, save_workspace};
use knotq_sync::{
    DocumentSyncCursor, LocalSyncState, MediaSyncCursor, PendingCrdtEdit, WorkspaceCrdtDocuments,
};

#[test]
fn capture_data_directory_fixture() {
    let Some(out) = std::env::var_os("KNOTQ_FIXTURE_OUT") else {
        eprintln!("KNOTQ_FIXTURE_OUT unset; nothing captured");
        return;
    };
    // Keep tokens in the file: a fixture must not depend on this machine's keychain.
    std::env::set_var("KNOTQ_DISABLE_KEYCHAIN", "1");

    let out = PathBuf::from(out);
    let _ = fs::remove_dir_all(&out);
    fs::create_dir_all(&out).unwrap();
    let workspace_path = out.join("workspace").join("workspace.json");

    let (workspace, asset) = build_workspace();
    save_workspace(&workspace_path, &workspace).unwrap();

    // An embedded image asset, referenced by one of the items above.
    let assets = out.join("workspace").join("assets").join("images");
    fs::create_dir_all(&assets).unwrap();
    fs::write(assets.join(format!("{asset}.png")), PNG_1PX).unwrap();

    save_app_settings(&out.join("settings.json"), &build_settings()).unwrap();

    // The CRDT documents, in this build's single-blob form.
    let documents = WorkspaceCrdtDocuments::try_new(&workspace).unwrap();
    let states = documents.document_states();
    save_crdt_state(&workspace_path, &states).unwrap();
    // Whichever form this build writes is the form the fixture should carry.
    assert!(
        crdt_state_path(&workspace_path).exists()
            || knotq_storage_json::crdt_state_dir(&workspace_path).is_dir(),
        "no CRDT state was written"
    );

    knotq_storage_json::save_local_sync_state(
        &workspace_path,
        &build_sync_state(&workspace, &states),
    )
    .unwrap();

    // The history store this build keeps beside the workspace.
    let _ = knotq_storage_json::record_workspace_snapshot(workspace_path.parent().unwrap());

    println!("captured fixture to {}", out.display());
    for entry in walk(&out) {
        println!("  {}", entry.strip_prefix(&out).unwrap().display());
    }
}

const PNG_1PX: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE,
    0x42, 0x60, 0x82,
];

fn build_workspace() -> (Workspace, uuid::Uuid) {
    let mut workspace = Workspace::new();
    let root = workspace.root;

    // A nested folder, so the scheme-file layout under `schemes/` is exercised.
    let mut work = Folder {
        id: FolderId::new(),
        name: "Work".into(),
        parent: Some(root),
        children: Vec::new(),
        expanded: true,
    };

    // 1. Plain notes with every marker and an indent.
    let mut notes = Scheme::new("Notes", 0);
    notes.items = vec![
        Item::new("A plain line"),
        {
            let mut item = Item::new("A bullet");
            item.marker = ItemMarker::Bullet;
            item
        },
        {
            let mut item = Item::new("An indented numbered line");
            item.marker = ItemMarker::Numbered;
            item.indent = 2;
            item
        },
        {
            let mut item = Item::new("Done task");
            item.marker = ItemMarker::Checkbox;
            item.state = vec![OccurrenceState {
                occurrence: OccurrenceId::Single,
                state: ItemState {
                    progress: -1,
                    notification_offset_secs: Some(900),
                },
            }];
            item
        },
        Item::new(
            "Markdown **bold**, __italic__, ==highlight==, ~~strike~~ and <angle> & ampersand",
        ),
        Item::new("Unicode: 日本語 — emoji 🧶 — quotes \"curly\" 'single'"),
    ];

    // 2. Scheduling: dates, recurrence, priority, per-occurrence overrides.
    let mut health = Scheme::new("Health", 3);
    let start = Utc.with_ymd_and_hms(2026, 8, 3, 15, 0, 0).unwrap();
    health.items = vec![
        {
            let mut item = Item::new("Weekly review");
            item.marker = ItemMarker::Checkbox;
            item.start = Some(start);
            item.end = Some(start + Duration::hours(1));
            item.available = Some(start - Duration::days(1));
            item.priority = Some(2);
            item.repeats = Some(Recurrence {
                rrules: vec!["FREQ=WEEKLY;BYDAY=MO,WE".into()],
                ..Default::default()
            });
            item
        },
        {
            let mut item = Item::new("A deadline");
            item.marker = ItemMarker::Checkbox;
            item.end = Some(start + Duration::days(30));
            item
        },
    ];

    // 3. Block content: one image line and one table line.
    let asset = uuid::Uuid::new_v4();
    let mut media = Scheme::new("Media", 5);
    let mut table = Table {
        columns: vec![TableColumn::new("Task"), TableColumn::new("Owner")],
        rows: Vec::new(),
    };
    let mut row = TableRow::new(2);
    row.cells[0].items = vec![Item::new("Ship it")];
    row.cells[1].items = vec![Item::new("me")];
    table.rows.push(row);
    media.items = vec![
        Item::new("Above the image"),
        {
            let mut item = Item::new("");
            item.content = ItemContent::Image(ImageInline {
                asset,
                format: ImageAssetFormat::Png,
                width: Some(1),
                height: Some(1),
            });
            item
        },
        {
            let mut item = Item::new("");
            item.content = ItemContent::Table(table);
            item
        },
    ];

    // 4. A read-only imported Google calendar.
    let mut imported = Scheme::new("Team calendar", 7);
    imported.gsync = true;
    imported.source = SchemeSource::ImportedCalendar(ImportedCalendarSource {
        provider: CalendarProvider::Google,
        account_id: "acct-1".into(),
        account_email: Some("user@example.com".into()),
        calendar_id: "primary".into(),
        sync_token: Some("tok-123".into()),
        read_only: true,
        last_synced_at: Some(start),
    });
    imported.items = vec![{
        let mut item = Item::new("Standup");
        item.start = Some(start);
        item.external = None;
        item
    }];

    // 5. An archived scheme, so `recently_deleted` + origins are populated.
    let archived = Scheme::new("Old project", 1);
    let archived_id = archived.id;

    for scheme in [notes, health, media, imported, archived] {
        let id = scheme.id;
        workspace.schemes.insert(id, scheme);
        if id == archived_id {
            continue;
        }
        workspace
            .folders
            .get_mut(&root)
            .unwrap()
            .children
            .push(NodeRef::Scheme(id));
    }
    // Put one scheme inside the nested folder instead of at the root.
    let moved = workspace
        .folders
        .get_mut(&root)
        .unwrap()
        .children
        .pop()
        .unwrap();
    work.children.push(moved);
    let work_id = work.id;
    workspace.folders.insert(work_id, work);
    workspace
        .folders
        .get_mut(&root)
        .unwrap()
        .children
        .push(NodeRef::Folder(work_id));
    workspace.mark_scheme_deleted_from(archived_id, root, 0);

    // 6. Two daily-queue days, one of them with completed rows.
    for (offset, text) in [(0, "today's row"), (1, "yesterday's row")] {
        let date = NaiveDate::from_ymd_opt(2026, 8, 20 - offset).unwrap();
        let mut daily = Scheme::new(date.to_string(), 0);
        let mut item = Item::new(text);
        item.marker = ItemMarker::Checkbox;
        if offset == 1 {
            item.state = vec![OccurrenceState {
                occurrence: OccurrenceId::Single,
                state: ItemState {
                    progress: -1,
                    notification_offset_secs: None,
                },
            }];
        }
        daily.items = vec![item];
        let id = daily.id;
        workspace.schemes.insert(id, daily);
        workspace.daily_queue.insert(date, id);
    }

    workspace.ensure_sync_metadata();
    (workspace, asset)
}

fn build_settings() -> AppSettings {
    AppSettings {
        theme_mode: ThemeMode::Dark,
        time_format: TimeFormat::TwentyFourHour,
        sync_account: Some(SyncAccountSettings {
            api_base: "https://api.knotq.com".into(),
            user_id: "11111111-1111-1111-1111-111111111111".into(),
            session_id: Some("22222222-2222-2222-2222-222222222222".into()),
            workspace_id: Some("33333333-3333-3333-3333-333333333333".into()),
            email: "user@example.com".into(),
            supports_sync: true,
            bearer_token: "FIXTURE-BEARER".into(),
            expires_at: Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0).unwrap(),
            refresh_token: Some("FIXTURE-REFRESH".into()),
            refresh_expires_at: None,
            account_status: None,
        }),
        google_accounts: vec![GoogleOAuthAccount {
            account_id: "acct-1".into(),
            email: Some("user@example.com".into()),
            client_id: "client-id".into(),
            access_token: "FIXTURE-GOOGLE-ACCESS".into(),
            refresh_token: "FIXTURE-GOOGLE-REFRESH".into(),
            expires_at: None,
            scope: "https://www.googleapis.com/auth/calendar.readonly".into(),
            token_source: GoogleTokenSource::OAuthRefreshToken,
            needs_reauth: false,
        }],
        ..Default::default()
    }
}

fn build_sync_state(
    workspace: &Workspace,
    states: &HashMap<DocumentId, std::sync::Arc<[u8]>>,
) -> LocalSyncState {
    let replica = ReplicaId::new();
    let mut state = LocalSyncState {
        workspace_id: Some(workspace.id),
        replica_id: Some(replica),
        server_url: Some("https://api.knotq.com".into()),
        ..Default::default()
    };
    for (index, document) in states.keys().enumerate() {
        state.document_cursors.insert(
            *document,
            DocumentSyncCursor {
                document: *document,
                kind: SyncDocumentKind::Scheme,
                last_pulled_sequence: index as u64 + 1,
                last_pushed_sequence: index as u64,
                epoch: 0,
            },
        );
    }
    // One unpushed edit: losing this on upgrade is silent data loss.
    if let Some(document) = states.keys().next() {
        state.push_pending(PendingCrdtEdit {
            operation_id: OperationId::new(),
            workspace_id: workspace.id,
            replica_id: replica,
            local_sequence: 41,
            created_at: Utc.with_ymd_and_hms(2026, 8, 19, 9, 30, 0).unwrap(),
            document: *document,
            kind: SyncDocumentKind::Scheme,
            update_v1: vec![1, 2, 3, 4, 5],
            touched_items: vec!["item-1".into()],
        });
        state.media_cursors.insert(
            "image.png".into(),
            MediaSyncCursor {
                image_name: "image.png".into(),
                document: *document,
                byte_length: 68,
                sha256: "0".repeat(64),
                uploaded_at: Utc.with_ymd_and_hms(2026, 8, 19, 9, 0, 0).unwrap(),
            },
        );
    }
    state
}

fn walk(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else {
            out.push(path);
        }
    }
    out.sort();
    out
}
