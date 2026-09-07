//! Read tools. Every one of these is a pure function of the indexed workspace
//! plus a clock — no mutation, nothing to undo, nothing to converge.

use chrono::NaiveDate;
use knotq_date_util::DateRange;
use knotq_index::search::{SearchOptions, SearchTarget};
use knotq_model::{ItemId, NodeRef, SchemeId};
use serde_json::{json, Value};

use crate::args::Args;
use crate::error::ToolError;
use crate::view::{daily_queue_dates, folder_summary, item_view, occurrence_view, scheme_summary, scheme_summary_with};
use crate::ToolContext;

pub fn list_schemes(args: &Args, ctx: &ToolContext) -> Result<Value, ToolError> {
    let include_archived = args.bool_or("include_archived", false)?;
    let ws = ctx.workspace;
    let daily = daily_queue_dates(ws);
    let mut schemes: Vec<_> = ws
        .schemes
        .values()
        .filter(|s| include_archived || !ws.recently_deleted.contains(&s.id))
        .map(|s| scheme_summary_with(ws, s, &daily))
        .collect();
    // HashMap iteration order is not stable between runs, and an agent that
    // re-lists a workspace and sees a different order will assume something
    // changed. Sort by where it lives, then by name.
    schemes.sort_by(|a, b| {
        a.folder_path
            .cmp(&b.folder_path)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.id.cmp(&b.id))
    });
    let mut folders: Vec<_> = ws
        .folders
        .values()
        .filter(|f| f.id != ws.root)
        .filter(|f| include_archived || !ws.recently_deleted_folders.contains(&f.id))
        .map(|f| folder_summary(ws, f))
        .collect();
    folders.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.id.cmp(&b.id)));
    Ok(json!({
        "root_folder_id": ws.root.to_string(),
        "folders": folders,
        "schemes": schemes,
    }))
}

pub fn read_scheme(args: &Args, ctx: &ToolContext) -> Result<Value, ToolError> {
    let scheme_id: SchemeId = args.req_id("scheme_id")?;
    let include_completed = args.bool_or("include_completed", true)?;
    let scheme = ctx.scheme(scheme_id)?;
    let items: Vec<_> = scheme
        .items
        .iter()
        .map(item_view)
        .filter(|v| include_completed || !v.done)
        .collect();
    Ok(json!({
        "scheme": scheme_summary(ctx.workspace, scheme),
        "items": items,
    }))
}

pub fn search(args: &Args, ctx: &ToolContext) -> Result<Value, ToolError> {
    let query = args.req_str("query")?;
    let limit = args.limit("limit", 25, 200)?;
    let options = SearchOptions {
        // The search index labels daily-queue hits with the app's own title for
        // that view. This crate has no locale, and the label is cosmetic here —
        // the ids in `target` are what the agent acts on.
        daily_queue_title: "Daily",
        daily_queue_marker_color: 0,
    };
    let hits = ctx
        .indexed
        .search_query(ctx.time_format, options)
        .run(query);
    let results: Vec<Value> = hits
        .iter()
        .take(limit)
        .map(|hit| {
            let (scheme_id, item_id): (Option<String>, Option<String>) = match &hit.target {
                SearchTarget::Scheme { scheme_id, item_id } => (
                    Some(scheme_id.to_string()),
                    item_id.as_ref().map(ItemId::to_string),
                ),
                SearchTarget::DailyQueue { scheme_id, item_id } => (
                    scheme_id.as_ref().map(SchemeId::to_string),
                    item_id.as_ref().map(ItemId::to_string),
                ),
                SearchTarget::Calendar => (None, None),
            };
            json!({
                "scheme_id": scheme_id,
                "item_id": item_id,
                "scheme_name": hit.scheme_name,
                "title": hit.title,
                "detail": hit.detail,
            })
        })
        .collect();
    Ok(json!({ "query": query, "results": results }))
}

pub fn list_upcoming(args: &Args, ctx: &ToolContext) -> Result<Value, ToolError> {
    let from = args.opt_datetime("from")?.unwrap_or(ctx.now);
    let limit = args.limit("limit", 25, 200)?;
    let events = ctx.indexed.calendar_query().upcoming(from, limit);
    Ok(json!({
        "from": from,
        "occurrences": events
            .iter()
            .map(|e| occurrence_view(e, ctx.workspace))
            .collect::<Vec<_>>(),
    }))
}

pub fn list_overdue(args: &Args, ctx: &ToolContext) -> Result<Value, ToolError> {
    let as_of = args.opt_datetime("as_of")?.unwrap_or(ctx.now);
    let limit = args.limit("limit", 50, 200)?;
    let events = ctx.indexed.calendar_query().overdue(as_of);
    Ok(json!({
        "as_of": as_of,
        "occurrences": events
            .iter()
            .take(limit)
            .map(|e| occurrence_view(e, ctx.workspace))
            .collect::<Vec<_>>(),
    }))
}

pub fn list_calendar(args: &Args, ctx: &ToolContext) -> Result<Value, ToolError> {
    let start = args.req_datetime("start")?;
    let end = args.req_datetime("end")?;
    if end <= start {
        return Err(ToolError::invalid("`end` must be after `start`"));
    }
    let limit = args.limit("limit", 200, 1000)?;
    let events = ctx.indexed.calendar_query().range(DateRange { start, end });
    let total = events.len();
    Ok(json!({
        "start": start,
        "end": end,
        // A range query can legitimately match more than the cap, and an agent
        // that cannot tell a full answer from a truncated one will summarise a
        // partial week as if it were the whole week.
        "truncated": total > limit,
        "total_matching": total,
        "occurrences": events
            .iter()
            .take(limit)
            .map(|e| occurrence_view(e, ctx.workspace))
            .collect::<Vec<_>>(),
    }))
}

pub fn get_daily_queue(args: &Args, ctx: &ToolContext) -> Result<Value, ToolError> {
    let date = match args.opt_str("date")? {
        None => ctx.today,
        Some(s) => s.parse::<NaiveDate>().map_err(|_| {
            ToolError::invalid("`date` must be a calendar date as YYYY-MM-DD")
        })?,
    };
    let Some(scheme_id) = ctx.workspace.daily_queue.get(&date).copied() else {
        return Ok(json!({ "date": date, "scheme": Value::Null, "items": [] }));
    };
    let Some(scheme) = ctx.workspace.schemes.get(&scheme_id) else {
        // The queue index names a scheme that is not in the workspace. Report
        // the day as empty rather than erroring: from the agent's side there is
        // nothing to act on either way, and a hard failure here would make an
        // ordinary "what's on today" call look like a server fault.
        return Ok(json!({ "date": date, "scheme": Value::Null, "items": [] }));
    };
    Ok(json!({
        "date": date,
        "scheme": scheme_summary(ctx.workspace, scheme),
        "items": scheme.items.iter().map(item_view).collect::<Vec<_>>(),
    }))
}

/// Folder ids in `parent`'s subtree, for reporting what a folder delete covers.
pub fn subtree_scheme_count(ctx: &ToolContext, folder: knotq_model::FolderId) -> usize {
    let mut count = 0;
    let mut stack = vec![folder];
    let mut guard = ctx.workspace.folders.len() + 1;
    while let Some(id) = stack.pop() {
        if guard == 0 {
            break;
        }
        guard -= 1;
        let Some(f) = ctx.workspace.folders.get(&id) else {
            continue;
        };
        for child in &f.children {
            match child {
                NodeRef::Folder(child_id) => stack.push(*child_id),
                NodeRef::Scheme(_) => count += 1,
            }
        }
    }
    count
}
