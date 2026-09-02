//! The KnotQ tool surface for MCP clients, as pure functions.
//!
//! This crate knows nothing about HTTP, GPUI, or where the workspace came
//! from. It turns a tool call into either a JSON answer or a
//! [`knotq_commands::Command`] for the caller to apply, which is what keeps
//! agent edits identical to human ones: the desktop app hands the command to
//! the same `KnotQApp::apply` a keystroke goes through, so it picks up the same
//! invariant checks, the same undo entry, the same notification reconcile and
//! the same CRDT write — and therefore converges with concurrent edits from
//! other devices for exactly the same reasons a typed edit does.
//!
//! Being pure is also what makes the surface testable: every mapping from tool
//! call to command is checked without a window, an event loop or a socket.

pub mod args;
pub mod error;
pub mod occurrence_handle;
pub mod protocol;
pub mod read;
pub mod schema;
pub mod view;
pub mod write;

use chrono::{DateTime, NaiveDate, Utc};
use knotq_commands::Command;
use knotq_index::IndexedWorkspace;
use knotq_model::{Folder, FolderId, Scheme, SchemeId, TimeFormat, Workspace};
use serde_json::Value;

pub use error::ToolError;
pub use schema::{tool_definitions, ToolDefinition};
pub use write::created_id;

/// Everything a tool call is evaluated against.
///
/// The workspace is borrowed *from the index* rather than passed separately, so
/// the two can never disagree — a tool that reads an occurrence from the
/// calendar index and then looks up its text in the workspace is guaranteed to
/// be looking at the same revision of both.
pub struct ToolContext<'a> {
    pub workspace: &'a Workspace,
    pub indexed: &'a IndexedWorkspace,
    pub now: DateTime<Utc>,
    /// The user's *local* calendar date. Passed in rather than derived, because
    /// "today" is a property of where the user is, and this crate has no
    /// business deciding that.
    pub today: NaiveDate,
    pub time_format: TimeFormat,
    /// When set, every write tool is refused before it can build a command.
    pub read_only: bool,
}

impl<'a> ToolContext<'a> {
    pub fn new(
        indexed: &'a IndexedWorkspace,
        now: DateTime<Utc>,
        today: NaiveDate,
        time_format: TimeFormat,
        read_only: bool,
    ) -> Self {
        Self {
            workspace: &indexed.workspace,
            indexed,
            now,
            today,
            time_format,
            read_only,
        }
    }

    pub fn scheme(&self, id: SchemeId) -> Result<&'a Scheme, ToolError> {
        self.workspace.schemes.get(&id).ok_or_else(|| {
            ToolError::not_found("no scheme with that id — call list_schemes for current ids")
        })
    }

    /// A scheme that may be edited. Refuses calendar-linked schemes up front so
    /// the agent gets a reason it can act on, rather than the generic invariant
    /// failure it would hit later inside `apply`.
    pub fn writable_scheme(&self, id: SchemeId) -> Result<&'a Scheme, ToolError> {
        let scheme = self.scheme(id)?;
        if scheme.is_read_only() {
            return Err(ToolError::refused(format!(
                "`{}` is a linked calendar and is read-only in KnotQ; edit it where it comes from",
                scheme.name
            )));
        }
        if self.workspace.recently_deleted.contains(&id) {
            return Err(ToolError::refused(format!(
                "`{}` is in the archive; the user must restore it before it can be edited",
                scheme.name
            )));
        }
        Ok(scheme)
    }

    pub fn folder(&self, id: FolderId) -> Result<&'a Folder, ToolError> {
        self.workspace.folders.get(&id).ok_or_else(|| {
            ToolError::not_found("no folder with that id — call list_schemes for current ids")
        })
    }
}

/// What a tool call produced.
#[derive(Debug)]
pub enum Outcome {
    /// A read. Nothing to apply.
    Read(Value),
    /// A write whose requested state the workspace was already in. Reported as
    /// a success with `changed: false`, so a retried call is safe and an agent
    /// can tell "I did that" from "that was already so".
    Unchanged(Value),
    /// A write. The caller applies `command`, then returns `response`.
    Write { command: Command, response: Value },
}

impl Outcome {
    pub fn unchanged(response: Value) -> Self {
        Outcome::Unchanged(response)
    }

    pub fn command(&self) -> Option<&Command> {
        match self {
            Outcome::Write { command, .. } => Some(command),
            _ => None,
        }
    }
}

/// Evaluate one tool call.
///
/// Does not mutate anything, ever — a write comes back as a command for the
/// caller to apply. See the module docs for why that separation is the whole
/// point.
pub fn call_tool(name: &str, arguments: Option<&Value>, ctx: &ToolContext) -> Result<Outcome, ToolError> {
    let definition =
        schema::find(name).ok_or_else(|| ToolError::UnknownTool(name.to_string()))?;
    if definition.writes && ctx.read_only {
        return Err(ToolError::refused(format!(
            "`{name}` changes the workspace, and this server is in read-only mode; \
             the user can turn writes on in KnotQ's settings"
        )));
    }
    let args = args::Args::new(arguments);
    match name {
        "list_schemes" => read::list_schemes(&args, ctx).map(Outcome::Read),
        "read_scheme" => read::read_scheme(&args, ctx).map(Outcome::Read),
        "search" => read::search(&args, ctx).map(Outcome::Read),
        "list_upcoming" => read::list_upcoming(&args, ctx).map(Outcome::Read),
        "list_overdue" => read::list_overdue(&args, ctx).map(Outcome::Read),
        "list_calendar" => read::list_calendar(&args, ctx).map(Outcome::Read),
        "get_daily_queue" => read::get_daily_queue(&args, ctx).map(Outcome::Read),

        "create_scheme" => write::create_scheme(&args, ctx),
        "rename_scheme" => write::rename_scheme(&args, ctx),
        "delete_scheme" => write::delete_scheme(&args, ctx),
        "create_folder" => write::create_folder(&args, ctx),
        "rename_folder" => write::rename_folder(&args, ctx),
        "delete_folder" => write::delete_folder(&args, ctx),
        "add_item" => write::add_item(&args, ctx),
        "update_item" => write::update_item(&args, ctx),
        "delete_item" => write::delete_item(&args, ctx),
        "move_item" => write::move_item(&args, ctx),
        "set_item_completed" => write::set_item_completed(&args, ctx),
        "set_item_recurrence" => write::set_item_recurrence(&args, ctx),

        // `schema::find` already matched the name, so a miss here means a tool
        // was advertised and never wired up. Fail loudly in development.
        other => {
            debug_assert!(false, "tool `{other}` is advertised but has no implementation");
            Err(ToolError::UnknownTool(other.to_string()))
        }
    }
}
