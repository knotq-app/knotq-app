#![allow(dead_code)]

use chrono::NaiveDate;
use knotq_model::{
    AppSettings, CalendarProvider, ImportedCalendarSource, Item, NodeRef, Scheme, SchemeSource,
    Workspace,
};
use knotq_state::AppState;

pub fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

pub fn test_state() -> AppState {
    AppState::new(
        Workspace::new(),
        AppSettings::default(),
        date(2026, 1, 1),
        date(2025, 12, 1),
        false,
        Default::default(),
        1,
    )
}

pub fn workspace_with_item(item: Item) -> Workspace {
    workspace_with_scheme_item(Scheme::new("General", 0), item)
}

pub fn read_only_workspace_with_item(item: Item) -> Workspace {
    let mut scheme = Scheme::new("Imported", 0);
    scheme.source = SchemeSource::ImportedCalendar(ImportedCalendarSource {
        provider: CalendarProvider::Google,
        account_id: "acct".into(),
        account_email: None,
        calendar_id: "cal".into(),
        sync_token: None,
        read_only: true,
        last_synced_at: None,
    });
    workspace_with_scheme_item(scheme, item)
}

pub fn workspace_with_scheme_item(mut scheme: Scheme, item: Item) -> Workspace {
    let mut workspace = Workspace::new();
    scheme.items.push(item);
    let scheme_id = scheme.id;
    workspace.schemes.insert(scheme_id, scheme);
    workspace
        .folders
        .get_mut(&workspace.root)
        .unwrap()
        .children
        .push(NodeRef::Scheme(scheme_id));
    workspace
}
