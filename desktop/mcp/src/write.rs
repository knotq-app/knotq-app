//! Write tools.
//!
//! Each of these produces a [`Command`] and nothing else — no mutation happens
//! here. That is deliberate: the app applies the command through the same
//! `KnotQApp::apply` path a keystroke takes, so an agent edit gets the same
//! invariant checks, undo entry, notification reconcile and CRDT write as a
//! human one, and converges with concurrent edits from other devices for
//! exactly the same reasons. Anything that mutated the workspace here would
//! bypass all of that.
//!
//! Two properties every write tool holds:
//!
//! * **Ids are minted here, before the command is applied.** The response can
//!   name what was created without waiting for the apply, and a retry that
//!   carries the same id is a no-op rather than a duplicate.
//! * **A call that asks for the state the workspace is already in succeeds and
//!   emits no command.** Models retry; retrying "mark this done" must not
//!   un-do it.

use knotq_commands::{Command, DateKind};
use knotq_model::{
    CalendarRecurrence, Item, ItemContent, ItemId, ItemMarker, NodeRef, OccurrenceId,
    OccurrenceState, Scheme, SchemeId,
};
use serde_json::json;

use crate::args::Args;
use crate::error::ToolError;
use crate::occurrence_handle;
use crate::view::item_view;
use crate::{Outcome, ToolContext};

/// Highest priority the UI offers. Rejecting anything above it keeps an agent
/// from writing a value no view can render.
const MAX_PRIORITY: u8 = 3;
/// Deepest nesting the editor allows.
const MAX_INDENT: u8 = 9;
/// Number of scheme colours in the palette.
const MAX_COLOR_INDEX: u8 = 11;

fn parse_marker(name: &str) -> Result<ItemMarker, ToolError> {
    ItemMarker::parse(name).map_err(|_| {
        ToolError::invalid("`marker` must be one of: blank, bullet, numbered, checkbox")
    })
}

// ── Schemes and folders ───────────────────────────────────────────────────

pub fn create_scheme(args: &Args, ctx: &ToolContext) -> Result<Outcome, ToolError> {
    let name = args.req_str("name")?.to_string();
    let folder = match args.opt_id("folder_id")? {
        Some(id) => ctx.folder(id)?.id,
        None => ctx.workspace.root,
    };
    let color_index = args.opt_u8("color_index", MAX_COLOR_INDEX)?.unwrap_or(0);
    // `CreateScheme` mints the id inside the command, so unlike the item tools
    // there is no id to report until the apply returns. The caller fills
    // `scheme_id` in afterwards with [`created_id`], because an agent that has
    // to re-list the workspace to find what it just created will sometimes pick
    // the wrong one — two schemes can share a name.
    Ok(Outcome::Write {
        command: Command::CreateScheme {
            folder,
            name: name.clone(),
            color_index,
            position: None,
        },
        response: json!({ "created": true, "name": name }),
    })
}

pub fn rename_scheme(args: &Args, ctx: &ToolContext) -> Result<Outcome, ToolError> {
    let id: SchemeId = args.req_id("scheme_id")?;
    let scheme = ctx.writable_scheme(id)?;
    let name = args.req_str("name")?.to_string();
    if scheme.name == name {
        return Ok(Outcome::unchanged(json!({ "scheme_id": id.to_string(), "name": name })));
    }
    Ok(Outcome::Write {
        command: Command::RenameScheme { id, name: name.clone() },
        response: json!({ "scheme_id": id.to_string(), "name": name }),
    })
}

pub fn delete_scheme(args: &Args, ctx: &ToolContext) -> Result<Outcome, ToolError> {
    let id: SchemeId = args.req_id("scheme_id")?;
    let scheme = ctx.scheme(id)?;
    if ctx.workspace.recently_deleted.contains(&id) {
        return Ok(Outcome::unchanged(
            json!({ "scheme_id": id.to_string(), "archived": true }),
        ));
    }
    let name = scheme.name.clone();
    Ok(Outcome::Write {
        command: Command::DeleteScheme { id },
        response: json!({ "scheme_id": id.to_string(), "name": name, "archived": true }),
    })
}

pub fn create_folder(args: &Args, ctx: &ToolContext) -> Result<Outcome, ToolError> {
    let name = args.req_str("name")?.to_string();
    let parent = match args.opt_id("parent_id")? {
        Some(id) => ctx.folder(id)?.id,
        None => ctx.workspace.root,
    };
    Ok(Outcome::Write {
        command: Command::CreateFolder {
            parent,
            name: name.clone(),
            position: None,
        },
        response: json!({ "created": true, "name": name }),
    })
}

pub fn rename_folder(args: &Args, ctx: &ToolContext) -> Result<Outcome, ToolError> {
    let id = args.req_id("folder_id")?;
    let folder = ctx.folder(id)?;
    let name = args.req_str("name")?.to_string();
    if folder.name == name {
        return Ok(Outcome::unchanged(json!({ "folder_id": id.to_string(), "name": name })));
    }
    Ok(Outcome::Write {
        command: Command::RenameFolder { id, name: name.clone() },
        response: json!({ "folder_id": id.to_string(), "name": name }),
    })
}

pub fn delete_folder(args: &Args, ctx: &ToolContext) -> Result<Outcome, ToolError> {
    let id = args.req_id("folder_id")?;
    let folder = ctx.folder(id)?;
    if id == ctx.workspace.root {
        return Err(ToolError::refused(
            "the workspace root folder cannot be deleted",
        ));
    }
    let name = folder.name.clone();
    let schemes = crate::read::subtree_scheme_count(ctx, id);
    Ok(Outcome::Write {
        command: Command::DeleteFolder { id },
        response: json!({
            "folder_id": id.to_string(),
            "name": name,
            "archived": true,
            "schemes_archived": schemes,
        }),
    })
}

// ── Items ─────────────────────────────────────────────────────────────────

pub fn add_item(args: &Args, ctx: &ToolContext) -> Result<Outcome, ToolError> {
    let scheme_id: SchemeId = args.req_id("scheme_id")?;
    let scheme = ctx.writable_scheme(scheme_id)?;
    let text = args.req_str("text")?.to_string();

    // Idempotent retry: the caller named an id that is already here, so the
    // previous attempt landed even if its response never arrived.
    if let Some(item_id) = args.opt_id::<ItemId>("item_id")? {
        if let Some(existing) = scheme.items.iter().find(|i| i.id == item_id) {
            return Ok(Outcome::unchanged(json!({
                "scheme_id": scheme_id.to_string(),
                "item": item_view(existing),
                "created": false,
            })));
        }
    }

    let mut item = Item::new(text);
    if let Some(id) = args.opt_id::<ItemId>("item_id")? {
        item.id = id;
    }
    item.indent = args.opt_u8("indent", MAX_INDENT)?.unwrap_or(0);
    item.start = args.opt_datetime("start")?;
    item.end = args.opt_datetime("end")?;
    item.available = args.opt_datetime("available")?;
    item.priority = args.opt_u8("priority", MAX_PRIORITY)?;
    if let Some(rrules) = args.opt_string_list("rrules")? {
        if !rrules.is_empty() {
            item.repeats = Some(CalendarRecurrence {
                rrules,
                ..Default::default()
            });
        }
    }
    item.marker = match args.opt_str("marker")? {
        Some(name) => parse_marker(name)?,
        // A dated line is something to be done, so it gets a checkbox; an
        // undated one is a note, so it gets a bullet. This matches what the
        // editor does when a user types a date onto a line.
        None if item.start.is_some() || item.end.is_some() => ItemMarker::Checkbox,
        None => ItemMarker::Bullet,
    };

    let position = match args.opt_usize("position")? {
        Some(p) if p > scheme.items.len() => {
            return Err(ToolError::invalid(format!(
                "`position` {p} is past the end of a scheme with {} lines",
                scheme.items.len()
            )))
        }
        Some(p) => p,
        None => scheme.items.len(),
    };

    let response = json!({
        "scheme_id": scheme_id.to_string(),
        "item": item_view(&item),
        "position": position,
        "created": true,
    });
    Ok(Outcome::Write {
        command: Command::InsertItem {
            scheme: scheme_id,
            position,
            item,
        },
        response,
    })
}

pub fn update_item(args: &Args, ctx: &ToolContext) -> Result<Outcome, ToolError> {
    let scheme_id: SchemeId = args.req_id("scheme_id")?;
    let scheme = ctx.writable_scheme(scheme_id)?;
    let item_id: ItemId = args.req_id("item_id")?;
    let item = find_item(scheme, item_id)?;

    let mut commands = Vec::new();

    if args.mentions("text") {
        let text = args
            .opt_str("text")?
            .ok_or_else(|| ToolError::invalid("`text` cannot be null — pass an empty string"))?;
        if !item.content.is_text() {
            return Err(ToolError::refused(
                "this line holds an image or a table, not text; its text cannot be set",
            ));
        }
        if item.content.as_text() != Some(text) {
            commands.push(Command::UpdateItemText {
                scheme: scheme_id,
                item: item_id,
                text: text.to_string(),
            });
        }
    }

    if args.mentions("marker") {
        // Clearing a marker means going back to a plain line, which is what
        // `blank` is — there is no "no marker" state below it.
        let marker = match args.opt_str("marker")? {
            Some(name) => parse_marker(name)?,
            None => ItemMarker::Blank,
        };
        if item.marker != marker {
            commands.push(Command::SetItemMarker {
                scheme: scheme_id,
                item: item_id,
                marker,
            });
        }
    }

    if args.mentions("indent") {
        let indent = args
            .opt_u8("indent", MAX_INDENT)?
            .ok_or_else(|| ToolError::invalid("`indent` cannot be null — pass 0"))?;
        if item.indent != indent {
            commands.push(Command::SetItemIndent {
                scheme: scheme_id,
                item: item_id,
                indent,
            });
        }
    }

    for (name, kind, current) in [
        ("start", DateKind::Start, item.start),
        ("end", DateKind::End, item.end),
        ("available", DateKind::Available, item.available),
    ] {
        if !args.mentions(name) {
            continue;
        }
        let date = args.opt_datetime(name)?;
        if current != date {
            commands.push(Command::SetItemDate {
                scheme: scheme_id,
                item: item_id,
                kind,
                date,
            });
        }
    }

    if args.mentions("priority") {
        let priority = args.opt_u8("priority", MAX_PRIORITY)?;
        if item.priority != priority {
            commands.push(Command::SetItemPriority {
                scheme: scheme_id,
                item: item_id,
                priority,
            });
        }
    }

    let response = json!({ "scheme_id": scheme_id.to_string(), "item_id": item_id.to_string() });
    match Command::from_vec(commands) {
        // Every requested field already holds the requested value. Report the
        // line as it stands so a retry still gets a useful answer.
        None => Ok(Outcome::unchanged(json!({
            "scheme_id": scheme_id.to_string(),
            "item": item_view(item),
        }))),
        Some(command) => Ok(Outcome::Write { command, response }),
    }
}

pub fn delete_item(args: &Args, ctx: &ToolContext) -> Result<Outcome, ToolError> {
    let scheme_id: SchemeId = args.req_id("scheme_id")?;
    let scheme = ctx.writable_scheme(scheme_id)?;
    let item_id: ItemId = args.req_id("item_id")?;
    // A delete whose target is already gone is the state the caller asked for.
    // Erroring would make an interrupted-and-retried delete look like a failure.
    if scheme.items.iter().all(|i| i.id != item_id) {
        return Ok(Outcome::unchanged(json!({
            "scheme_id": scheme_id.to_string(),
            "item_id": item_id.to_string(),
            "deleted": true,
        })));
    }
    Ok(Outcome::Write {
        command: Command::DeleteItem {
            scheme: scheme_id,
            item: item_id,
        },
        response: json!({
            "scheme_id": scheme_id.to_string(),
            "item_id": item_id.to_string(),
            "deleted": true,
        }),
    })
}

pub fn move_item(args: &Args, ctx: &ToolContext) -> Result<Outcome, ToolError> {
    let scheme_id: SchemeId = args.req_id("scheme_id")?;
    let scheme = ctx.writable_scheme(scheme_id)?;
    let item_id: ItemId = args.req_id("item_id")?;
    let from = scheme
        .item_index(item_id)
        .ok_or_else(|| ToolError::not_found("no such line in this scheme"))?;
    let to = args
        .opt_usize("position")?
        .ok_or_else(|| ToolError::invalid("`position` is required"))?;
    let last = scheme.items.len().saturating_sub(1);
    if to > last {
        return Err(ToolError::invalid(format!(
            "`position` {to} is past the last line ({last})"
        )));
    }
    if from == to {
        return Ok(Outcome::unchanged(json!({
            "scheme_id": scheme_id.to_string(),
            "item_id": item_id.to_string(),
            "position": to,
        })));
    }
    Ok(Outcome::Write {
        command: Command::ReorderItem {
            scheme: scheme_id,
            from,
            to,
        },
        response: json!({
            "scheme_id": scheme_id.to_string(),
            "item_id": item_id.to_string(),
            "from": from,
            "position": to,
        }),
    })
}

pub fn set_item_completed(args: &Args, ctx: &ToolContext) -> Result<Outcome, ToolError> {
    let scheme_id: SchemeId = args.req_id("scheme_id")?;
    let scheme = ctx.writable_scheme(scheme_id)?;
    let item_id: ItemId = args.req_id("item_id")?;
    let item = find_item(scheme, item_id)?;
    let completed = args
        .opt_bool("completed")?
        .ok_or_else(|| ToolError::invalid("`completed` is required"))?;

    let handle = args.opt_str("occurrence")?.unwrap_or("single");
    let occurrence = occurrence_handle::decode(handle).ok_or_else(|| {
        ToolError::invalid(
            "`occurrence` is not a handle this server issued — pass one back verbatim from \
             list_upcoming, list_overdue or list_calendar, or omit it",
        )
    })?;
    if item.repeats.is_some() && occurrence.is_single() {
        return Err(ToolError::invalid(
            "this line repeats, so completing it needs the `occurrence` handle of the instance \
             you mean — take one from list_upcoming, list_overdue or list_calendar",
        ));
    }

    let currently_done = occurrence_is_done(item, &occurrence);
    // The underlying command is a *toggle*; this tool is a *set*. Emitting it
    // only on a real difference is what makes a retry safe, and what stops two
    // agents racing on the same item from flipping it back open.
    if currently_done == completed {
        return Ok(Outcome::unchanged(json!({
            "scheme_id": scheme_id.to_string(),
            "item_id": item_id.to_string(),
            "occurrence": occurrence_handle::encode(&occurrence),
            "completed": completed,
        })));
    }
    Ok(Outcome::Write {
        command: Command::ToggleOccurrence {
            scheme: scheme_id,
            item: item_id,
            occurrence: occurrence.clone(),
        },
        response: json!({
            "scheme_id": scheme_id.to_string(),
            "item_id": item_id.to_string(),
            "occurrence": occurrence_handle::encode(&occurrence),
            "completed": completed,
        }),
    })
}

pub fn set_item_recurrence(args: &Args, ctx: &ToolContext) -> Result<Outcome, ToolError> {
    let scheme_id: SchemeId = args.req_id("scheme_id")?;
    let scheme = ctx.writable_scheme(scheme_id)?;
    let item_id: ItemId = args.req_id("item_id")?;
    let item = find_item(scheme, item_id)?;
    if !args.mentions("rrules") {
        return Err(ToolError::invalid(
            "`rrules` is required — pass the rules to set, or null to stop the line repeating",
        ));
    }
    let rrules = args.opt_string_list("rrules")?.unwrap_or_default();
    let repeats = if rrules.is_empty() {
        None
    } else {
        if item.start.is_none() {
            return Err(ToolError::refused(
                "a line needs a start date before it can repeat — set `start` first",
            ));
        }
        Some(CalendarRecurrence {
            rrules,
            // Keep whatever exception dates and per-occurrence overrides the
            // line already carries: those record edits the user made to
            // individual instances, and dropping them would silently resurrect
            // occurrences they had deleted or moved.
            rdates: item.repeats.as_ref().map(|r| r.rdates.clone()).unwrap_or_default(),
            exdates: item.repeats.as_ref().map(|r| r.exdates.clone()).unwrap_or_default(),
            overrides: item
                .repeats
                .as_ref()
                .map(|r| r.overrides.clone())
                .unwrap_or_default(),
            raw_import: item.repeats.as_ref().and_then(|r| r.raw_import.clone()),
        })
    };
    if item.repeats == repeats {
        return Ok(Outcome::unchanged(json!({
            "scheme_id": scheme_id.to_string(),
            "item": item_view(item),
        })));
    }
    let response = json!({
        "scheme_id": scheme_id.to_string(),
        "item_id": item_id.to_string(),
        "rrules": repeats.as_ref().map(|r| r.rrules.clone()).unwrap_or_default(),
    });
    Ok(Outcome::Write {
        command: Command::SetItemRecurrence {
            scheme: scheme_id,
            item: item_id,
            repeats,
        },
        response,
    })
}

// ── Shared helpers ────────────────────────────────────────────────────────

fn find_item(scheme: &Scheme, item: ItemId) -> Result<&Item, ToolError> {
    scheme
        .items
        .iter()
        .find(|i| i.id == item)
        .ok_or_else(|| ToolError::not_found("no such line in this scheme"))
}

fn occurrence_is_done(item: &Item, occurrence: &OccurrenceId) -> bool {
    item.state
        .iter()
        .find(|s: &&OccurrenceState| &s.occurrence == occurrence)
        .map(|s| s.state.is_done())
        .unwrap_or(false)
}

/// The folder a scheme sits in, if any — used by tools that report placement.
pub fn parent_folder(ctx: &ToolContext, scheme: SchemeId) -> Option<knotq_model::FolderId> {
    ctx.workspace
        .folders
        .values()
        .find(|f| f.children.contains(&NodeRef::Scheme(scheme)))
        .map(|f| f.id)
}

/// Text of a line, for building a response about it without cloning the item.
pub fn item_text(item: &Item) -> &str {
    match &item.content {
        ItemContent::Text { text } => text.as_str(),
        _ => "",
    }
}

/// The id of the thing a command created, read back out of its inverse.
///
/// `CreateScheme` and `CreateFolder` mint their own ids, so the only place the
/// new id exists after an apply is the receipt's inverse — a `DeleteScheme` /
/// `DeleteFolder` naming it. Pulling it out here keeps that knowledge in the
/// same crate as the tools whose responses need it, and keeps the transport
/// from having to know which commands create things.
pub fn created_id(inverse: &Command) -> Option<(&'static str, String)> {
    match inverse {
        Command::DeleteScheme { id } => Some(("scheme_id", id.to_string())),
        Command::DeleteFolder { id } => Some(("folder_id", id.to_string())),
        // A batch's inverse is its commands reversed; the creation, if any, is
        // the one whose inverse deletes something.
        Command::Batch(commands) => commands.iter().find_map(created_id),
        _ => None,
    }
}
