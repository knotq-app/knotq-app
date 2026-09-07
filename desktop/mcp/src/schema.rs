//! The advertised tool surface.
//!
//! Descriptions here are the only documentation the model gets, so they carry
//! the things it cannot infer from a JSON schema: which ids are opaque, what a
//! missing field means versus an explicit null, and which calls are refused on
//! a read-only scheme.

use serde::Serialize;
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize)]
pub struct ToolDefinition {
    pub name: &'static str,
    pub description: &'static str,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
    /// Not part of the MCP payload — used by the server to refuse writes when
    /// the read-only toggle is on, without having to know each tool by name.
    #[serde(skip)]
    pub writes: bool,
}

fn object(properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

const ID_NOTE: &str = "Ids are opaque — pass back exactly what a read tool returned.";

pub fn tool_definitions() -> Vec<ToolDefinition> {
    vec![
        // ── Reads ──────────────────────────────────────────────────────────
        ToolDefinition {
            name: "list_schemes",
            description: "List every scheme in the KnotQ workspace with its folder path, item \
                          counts, and read-only state. Use this first whenever the user asks \
                          about KnotQ or their plans; it discovers the ids other tools need.",
            input_schema: object(
                json!({
                    "include_archived": {
                        "type": "boolean",
                        "description": "Include schemes in the archive. Defaults to false.",
                    },
                }),
                &[],
            ),
            writes: false,
        },
        ToolDefinition {
            name: "read_scheme",
            description: "Read a scheme's lines in order, with markers, indentation, dates, \
                          priorities and completion. This is the full text of the document.",
            input_schema: object(
                json!({
                    "scheme_id": { "type": "string", "description": ID_NOTE },
                    "include_completed": {
                        "type": "boolean",
                        "description": "Include completed lines. Defaults to true.",
                    },
                }),
                &["scheme_id"],
            ),
            writes: false,
        },
        ToolDefinition {
            name: "search",
            description: "Full-text search across every scheme. Returns matching lines with the \
                          scheme and item ids needed to read or edit them.",
            input_schema: object(
                json!({
                    "query": { "type": "string" },
                    "limit": { "type": "integer", "description": "Default 25, max 200." },
                }),
                &["query"],
            ),
            writes: false,
        },
        ToolDefinition {
            name: "list_upcoming",
            description: "Scheduled occurrences starting at or after a time, soonest first. \
                          This is the agenda view: use it for \"what's next\" and \"what's on \
                          Thursday\". Repeating items are expanded into individual occurrences.",
            input_schema: object(
                json!({
                    "from": {
                        "type": "string",
                        "description": "RFC 3339 instant to start from. Defaults to now.",
                    },
                    "limit": { "type": "integer", "description": "Default 25, max 200." },
                }),
                &[],
            ),
            writes: false,
        },
        ToolDefinition {
            name: "list_overdue",
            description: "Occurrences whose time has passed and which are not complete.",
            input_schema: object(
                json!({
                    "as_of": {
                        "type": "string",
                        "description": "RFC 3339 instant to measure against. Defaults to now.",
                    },
                    "limit": { "type": "integer", "description": "Default 50, max 200." },
                }),
                &[],
            ),
            writes: false,
        },
        ToolDefinition {
            name: "list_calendar",
            description: "Every occurrence overlapping an explicit time range, for a calendar \
                          view. Unlike list_upcoming this includes past and completed items.",
            input_schema: object(
                json!({
                    "start": { "type": "string", "description": "RFC 3339 instant, inclusive." },
                    "end": { "type": "string", "description": "RFC 3339 instant, exclusive." },
                    "limit": { "type": "integer", "description": "Default 200, max 1000." },
                }),
                &["start", "end"],
            ),
            writes: false,
        },
        ToolDefinition {
            name: "get_daily_queue",
            description: "The daily queue scheme for a given day — the user's plan for that \
                          date. Returns null if no queue exists for it yet.",
            input_schema: object(
                json!({
                    "date": {
                        "type": "string",
                        "description": "Local calendar date as YYYY-MM-DD. Defaults to today.",
                    },
                }),
                &[],
            ),
            writes: false,
        },
        // ── Writes ─────────────────────────────────────────────────────────
        ToolDefinition {
            name: "create_scheme",
            description: "Create an empty scheme. Returns its new id.",
            input_schema: object(
                json!({
                    "name": { "type": "string" },
                    "folder_id": {
                        "type": "string",
                        "description": "Parent folder. Defaults to the workspace root.",
                    },
                    "color_index": { "type": "integer", "description": "0-11. Defaults to 0." },
                }),
                &["name"],
            ),
            writes: true,
        },
        ToolDefinition {
            name: "rename_scheme",
            description: "Rename a scheme.",
            input_schema: object(
                json!({
                    "scheme_id": { "type": "string", "description": ID_NOTE },
                    "name": { "type": "string" },
                }),
                &["scheme_id", "name"],
            ),
            writes: true,
        },
        ToolDefinition {
            name: "delete_scheme",
            description: "Move a scheme to the archive. Reversible by the user from the app; \
                          this never destroys data permanently.",
            input_schema: object(
                json!({ "scheme_id": { "type": "string", "description": ID_NOTE } }),
                &["scheme_id"],
            ),
            writes: true,
        },
        ToolDefinition {
            name: "create_folder",
            description: "Create a folder. Returns its new id.",
            input_schema: object(
                json!({
                    "name": { "type": "string" },
                    "parent_id": {
                        "type": "string",
                        "description": "Parent folder. Defaults to the workspace root.",
                    },
                }),
                &["name"],
            ),
            writes: true,
        },
        ToolDefinition {
            name: "rename_folder",
            description: "Rename a folder.",
            input_schema: object(
                json!({
                    "folder_id": { "type": "string", "description": ID_NOTE },
                    "name": { "type": "string" },
                }),
                &["folder_id", "name"],
            ),
            writes: true,
        },
        ToolDefinition {
            name: "delete_folder",
            description: "Move a folder and everything inside it to the archive. Reversible by \
                          the user from the app; this never destroys data permanently.",
            input_schema: object(
                json!({ "folder_id": { "type": "string", "description": ID_NOTE } }),
                &["folder_id"],
            ),
            writes: true,
        },
        ToolDefinition {
            name: "add_item",
            description: "Add a line to a scheme. Everything but the text is optional. \
                          Supply `item_id` to make the call idempotent: retrying with the same \
                          id will not add a second copy.",
            input_schema: object(
                json!({
                    "scheme_id": { "type": "string", "description": ID_NOTE },
                    "text": { "type": "string" },
                    "position": {
                        "type": "integer",
                        "description": "0-based line index. Defaults to the end of the scheme.",
                    },
                    "marker": {
                        "type": "string",
                        "enum": ["blank", "bullet", "numbered", "checkbox"],
                        "description": "Defaults to checkbox when the line has a date, else bullet.",
                    },
                    "indent": { "type": "integer", "description": "Nesting depth, 0-9. Defaults to 0." },
                    "start": { "type": "string", "description": "RFC 3339 instant with offset." },
                    "end": { "type": "string", "description": "RFC 3339 instant with offset." },
                    "available": {
                        "type": "string",
                        "description": "RFC 3339 instant the item becomes actionable.",
                    },
                    "priority": { "type": "integer", "description": "0-3, higher is more urgent." },
                    "rrules": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "iCalendar RRULE lines, e.g. FREQ=WEEKLY;BYDAY=MO,WE.",
                    },
                    "item_id": {
                        "type": "string",
                        "description": "Optional client-chosen UUID for idempotent retries.",
                    },
                }),
                &["scheme_id", "text"],
            ),
            writes: true,
        },
        ToolDefinition {
            name: "update_item",
            description: "Change fields on an existing line. Omit a field to leave it alone; \
                          pass it as null to clear it. Only text lines can have their text \
                          changed — image and table lines refuse it.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "scheme_id": { "type": "string", "description": ID_NOTE },
                    "item_id": { "type": "string", "description": ID_NOTE },
                    "text": { "type": "string" },
                    "marker": {
                        "type": ["string", "null"],
                        "enum": ["blank", "bullet", "numbered", "checkbox", null],
                    },
                    "indent": { "type": "integer" },
                    "start": { "type": ["string", "null"] },
                    "end": { "type": ["string", "null"] },
                    "available": { "type": ["string", "null"] },
                    "priority": { "type": ["integer", "null"] },
                },
                "required": ["scheme_id", "item_id"],
                "additionalProperties": false,
            }),
            writes: true,
        },
        ToolDefinition {
            name: "delete_item",
            description: "Remove a line from a scheme. The user can undo this in the app.",
            input_schema: object(
                json!({
                    "scheme_id": { "type": "string", "description": ID_NOTE },
                    "item_id": { "type": "string", "description": ID_NOTE },
                }),
                &["scheme_id", "item_id"],
            ),
            writes: true,
        },
        ToolDefinition {
            name: "move_item",
            description: "Move a line to a different position within its scheme.",
            input_schema: object(
                json!({
                    "scheme_id": { "type": "string", "description": ID_NOTE },
                    "item_id": { "type": "string", "description": ID_NOTE },
                    "position": { "type": "integer", "description": "0-based target line index." },
                }),
                &["scheme_id", "item_id", "position"],
            ),
            writes: true,
        },
        ToolDefinition {
            name: "set_item_completed",
            description: "Mark a line complete or incomplete. Idempotent: setting it to what it \
                          already is succeeds and changes nothing. For a repeating item pass the \
                          `occurrence` handle from a calendar tool to complete one instance; \
                          omit it to act on a non-repeating line.",
            input_schema: object(
                json!({
                    "scheme_id": { "type": "string", "description": ID_NOTE },
                    "item_id": { "type": "string", "description": ID_NOTE },
                    "completed": { "type": "boolean" },
                    "occurrence": {
                        "type": "string",
                        "description": "Opaque occurrence handle from list_upcoming, \
                                        list_overdue or list_calendar. Pass it back verbatim.",
                    },
                }),
                &["scheme_id", "item_id", "completed"],
            ),
            writes: true,
        },
        ToolDefinition {
            name: "set_item_recurrence",
            description: "Set or clear a line's repeat rule. Pass `rrules` as iCalendar RRULE \
                          lines, or null to stop it repeating.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "scheme_id": { "type": "string", "description": ID_NOTE },
                    "item_id": { "type": "string", "description": ID_NOTE },
                    "rrules": {
                        "type": ["array", "null"],
                        "items": { "type": "string" },
                        "description": "e.g. [\"FREQ=WEEKLY;BYDAY=MO,WE\"]. Null clears the repeat.",
                    },
                },
                "required": ["scheme_id", "item_id", "rrules"],
                "additionalProperties": false,
            }),
            writes: true,
        },
    ]
}

pub fn find(name: &str) -> Option<ToolDefinition> {
    tool_definitions().into_iter().find(|t| t.name == name)
}
