//! A small workspace to run tools against, plus the two things every test in
//! this crate does: evaluate a tool call, and apply whatever command it
//! produced so the next call sees the result.

#![allow(dead_code)]

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use knotq_commands::{CommandOrigin, WorkspaceCommandExt};
use knotq_index::IndexedWorkspace;
use knotq_mcp::{call_tool, Outcome, ToolContext, ToolError};
use knotq_model::{
    CalendarProvider, CalendarRecurrence, FolderId, ImportedCalendarSource, Item, ItemMarker,
    NodeRef, Scheme, SchemeId, SchemeSource, TimeFormat, Workspace,
};
use serde_json::Value;

pub fn at(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(y, m, d, h, min, 0).unwrap()
}

pub const NOW: fn() -> DateTime<Utc> = || at(2026, 9, 1, 12, 0);

pub fn today() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 9, 1).unwrap()
}

/// A workspace plus the ids a test needs to name things in it.
pub struct Fixture {
    pub workspace: Workspace,
    pub folder: FolderId,
    /// An ordinary editable scheme with four lines.
    pub notes: SchemeId,
    /// A linked Google calendar: read-only to users and agents alike.
    pub calendar: SchemeId,
    /// An archived scheme.
    pub archived: SchemeId,
}

impl Fixture {
    pub fn new() -> Self {
        let mut workspace = Workspace::new();
        let root = workspace.root;

        let folder = FolderId::new();
        workspace.folders.insert(
            folder,
            knotq_model::Folder {
                id: folder,
                name: "Work".into(),
                parent: Some(root),
                children: Vec::new(),
                expanded: true,
            },
        );
        workspace
            .folders
            .get_mut(&root)
            .unwrap()
            .children
            .push(NodeRef::Folder(folder));

        let mut notes = Scheme::new("Notes", 1);
        notes.items = vec![
            {
                let mut i = Item::new("write the proposal");
                i.marker = ItemMarker::Checkbox;
                i.start = Some(at(2026, 9, 2, 9, 0));
                i
            },
            {
                let mut i = Item::new("overdue thing");
                i.marker = ItemMarker::Checkbox;
                i.start = Some(at(2026, 8, 20, 9, 0));
                i
            },
            {
                let mut i = Item::new("standup");
                i.marker = ItemMarker::Checkbox;
                i.start = Some(at(2026, 9, 2, 9, 30));
                i.repeats = Some(CalendarRecurrence {
                    rrules: vec!["FREQ=DAILY".into()],
                    ..Default::default()
                });
                i
            },
            Item::new("a plain note"),
        ];
        let notes_id = notes.id;

        let mut calendar = Scheme::new("Team calendar", 2);
        calendar.source = SchemeSource::ImportedCalendar(ImportedCalendarSource {
            provider: CalendarProvider::Google,
            account_id: "account".into(),
            account_email: None,
            calendar_id: "calendar".into(),
            sync_token: None,
            read_only: true,
            last_synced_at: None,
        });
        calendar.items = vec![Item::new("all hands")];
        let calendar_id = calendar.id;

        let archived = Scheme::new("Old project", 3);
        let archived_id = archived.id;

        for scheme in [notes, calendar, archived] {
            let id = scheme.id;
            workspace.schemes.insert(id, scheme);
            workspace
                .folders
                .get_mut(&folder)
                .unwrap()
                .children
                .push(NodeRef::Scheme(id));
        }
        workspace.recently_deleted.push(archived_id);

        Self {
            workspace,
            folder,
            notes: notes_id,
            calendar: calendar_id,
            archived: archived_id,
        }
    }

    pub fn item_id(&self, scheme: SchemeId, index: usize) -> knotq_model::ItemId {
        self.workspace.schemes[&scheme].items[index].id
    }

    pub fn scheme(&self, id: SchemeId) -> &Scheme {
        &self.workspace.schemes[&id]
    }

    /// Evaluate a tool call. Nothing is applied — use [`Fixture::run`] for that.
    pub fn call(&self, name: &str, args: Value) -> Result<Outcome, ToolError> {
        self.call_as(name, args, false)
    }

    pub fn call_as(&self, name: &str, args: Value, read_only: bool) -> Result<Outcome, ToolError> {
        let indexed = IndexedWorkspace::build(self.workspace.clone());
        let ctx = ToolContext::new(&indexed, NOW(), today(), TimeFormat::default(), read_only);
        call_tool(name, Some(&args), &ctx)
    }

    /// Evaluate a tool call *and* apply any command it produced, exactly as the
    /// desktop app would — through `WorkspaceCommandExt` with an agent origin,
    /// so the same invariants run.
    pub fn run(&mut self, name: &str, args: Value) -> Result<Value, ToolError> {
        match self.call(name, args)? {
            Outcome::Read(v) | Outcome::Unchanged(v) => Ok(v),
            Outcome::Write { command, response } => {
                self.workspace
                    .apply_with_origin(command, CommandOrigin::Agent)
                    .map_err(|e| ToolError::refused(e.to_string()))?;
                Ok(response)
            }
        }
    }

    /// A read tool's JSON, unwrapped.
    pub fn read(&self, name: &str, args: Value) -> Value {
        match self.call(name, args).expect("read tool failed") {
            Outcome::Read(v) => v,
            other => panic!("expected a read outcome, got {other:?}"),
        }
    }
}

pub fn is_unchanged(outcome: &Outcome) -> bool {
    matches!(outcome, Outcome::Unchanged(_))
}
