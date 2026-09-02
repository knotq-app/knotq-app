//! The JSON shapes the agent sees.
//!
//! Deliberately *not* `#[derive(Serialize)]` on the model types. The on-disk
//! model is tuned for storage and CRDT merge (flattened occurrence state,
//! marker families, sync metadata, image blobs) and is free to change shape
//! whenever a migration says so; this is a stable, narrow projection built for
//! reading by a language model. Keeping it separate means a model refactor
//! can't silently change the API, and the wire format never leaks a field the
//! agent has no business writing back.

use std::collections::HashMap;

use chrono::{DateTime, NaiveDate, Utc};
use knotq_index::calendar::OccurrenceWithContext;
use knotq_model::{
    Folder, Item, ItemContent, ItemMarker, NodeRef, Occurrence, Scheme, SchemeId, Workspace,
};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct SchemeSummary {
    pub id: String,
    pub name: String,
    pub folder_path: Vec<String>,
    pub item_count: usize,
    pub open_count: usize,
    /// True for calendar-linked schemes (an imported Google calendar). Every
    /// write tool refuses these, so say so up front rather than letting the
    /// agent discover it by being rejected.
    pub read_only: bool,
    pub archived: bool,
    /// Set when this scheme is a *day's* plan rather than a standing document.
    /// A long-lived workspace accumulates one per planned day, and without this
    /// they are indistinguishable from real documents in a listing — an agent
    /// asked to "add this to my notes" would have hundreds of equally plausible
    /// candidates.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub daily_queue_date: Option<NaiveDate>,
}

#[derive(Debug, Serialize)]
pub struct FolderSummary {
    pub id: String,
    pub name: String,
    pub path: Vec<String>,
    pub parent: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ItemView {
    pub id: String,
    /// The line's text. An image or table line has no text; `kind` says which.
    pub text: String,
    /// `"text"`, `"image"` or `"table"`. Only `"text"` lines can be edited by
    /// the text tools — the others are whole-line block objects.
    pub kind: &'static str,
    pub marker: &'static str,
    pub indent: u8,
    pub done: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<u8>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub recurrence: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct OccurrenceView {
    pub scheme_id: String,
    pub scheme_name: String,
    pub item_id: String,
    pub text: String,
    /// Opaque handle for this specific instance of a repeating item. Pass it
    /// back verbatim to `set_item_completed`; do not construct one.
    pub occurrence: String,
    pub kind: String,
    pub done: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end: Option<DateTime<Utc>>,
}

pub fn item_kind(content: &ItemContent) -> &'static str {
    match content {
        ItemContent::Text { .. } => "text",
        ItemContent::Image(_) => "image",
        ItemContent::Table(_) => "table",
    }
}

pub fn marker_name(marker: ItemMarker) -> &'static str {
    marker.as_str()
}

pub fn item_view(item: &Item) -> ItemView {
    ItemView {
        id: item.id.to_string(),
        text: item.content.as_text().unwrap_or_default().to_string(),
        kind: item_kind(&item.content),
        marker: marker_name(item.marker),
        indent: item.indent,
        done: item_is_done(item),
        start: item.start,
        end: item.end,
        available: item.available,
        priority: item.priority,
        recurrence: item
            .repeats
            .as_ref()
            .map(|r| r.rrules.clone())
            .unwrap_or_default(),
    }
}

/// Completion of the item's *base* occurrence. A repeating item has one state
/// slot per occurrence; this reports the non-recurring one, which is the only
/// one `read_scheme` can meaningfully name. Per-occurrence completion is read
/// through the calendar tools, which carry an occurrence handle.
pub fn item_is_done(item: &Item) -> bool {
    item.state
        .iter()
        .find(|s| s.occurrence.is_single())
        .map(|s| s.state.is_done())
        .unwrap_or(false)
}

pub fn occurrence_kind(occurrence: &Occurrence) -> String {
    format!("{:?}", occurrence.kind).to_lowercase()
}

pub fn occurrence_view(hit: &OccurrenceWithContext, workspace: &Workspace) -> OccurrenceView {
    let text = workspace
        .schemes
        .get(&hit.scheme_id)
        .and_then(|scheme| scheme.items.iter().find(|i| i.id == hit.item_id))
        .and_then(|item| item.content.as_text())
        .unwrap_or_default()
        .to_string();
    OccurrenceView {
        scheme_id: hit.scheme_id.to_string(),
        scheme_name: hit.scheme_name.clone(),
        item_id: hit.item_id.to_string(),
        text,
        occurrence: crate::occurrence_handle::encode(&hit.occurrence.id),
        kind: occurrence_kind(&hit.occurrence),
        done: hit.occurrence.state.is_done(),
        start: hit.occurrence.start,
        end: hit.occurrence.end,
    }
}

/// Folder names from the root down to (but not including) the node itself.
pub fn folder_path(workspace: &Workspace, mut folder: Option<knotq_model::FolderId>) -> Vec<String> {
    let mut path = Vec::new();
    // The root folder is the workspace container, not a user-visible folder, so
    // it is never part of a path. Bound the walk by folder count: a cycle here
    // would otherwise hang the whole app on the main thread.
    let mut guard = workspace.folders.len() + 1;
    while let Some(id) = folder {
        if guard == 0 {
            break;
        }
        guard -= 1;
        if id == workspace.root {
            break;
        }
        let Some(f) = workspace.folders.get(&id) else {
            break;
        };
        path.push(f.name.clone());
        folder = f.parent;
    }
    path.reverse();
    path
}

pub fn folder_summary(workspace: &Workspace, folder: &Folder) -> FolderSummary {
    FolderSummary {
        id: folder.id.to_string(),
        name: folder.name.clone(),
        path: folder_path(workspace, Some(folder.id)),
        parent: folder.parent.map(|p| p.to_string()),
    }
}

/// Which schemes are a day's plan, keyed by scheme.
///
/// Built once per listing rather than looked up per scheme, so a workspace with
/// a year of planned days does not turn a listing into a quadratic scan.
pub fn daily_queue_dates(workspace: &Workspace) -> HashMap<SchemeId, NaiveDate> {
    workspace
        .daily_queue
        .iter()
        .map(|(date, scheme)| (*scheme, *date))
        .collect()
}

pub fn scheme_summary(workspace: &Workspace, scheme: &Scheme) -> SchemeSummary {
    scheme_summary_with(workspace, scheme, &daily_queue_dates(workspace))
}

pub fn scheme_summary_with(
    workspace: &Workspace,
    scheme: &Scheme,
    daily: &HashMap<SchemeId, NaiveDate>,
) -> SchemeSummary {
    SchemeSummary {
        id: scheme.id.to_string(),
        name: scheme.name.clone(),
        folder_path: folder_path(workspace, parent_of_scheme(workspace, scheme.id)),
        item_count: scheme.items.len(),
        open_count: scheme
            .items
            .iter()
            .filter(|i| !item_is_done(i) && !i.content.as_text().unwrap_or_default().is_empty())
            .count(),
        read_only: scheme.is_read_only(),
        archived: workspace.recently_deleted.contains(&scheme.id),
        daily_queue_date: daily.get(&scheme.id).copied(),
    }
}

pub fn parent_of_scheme(
    workspace: &Workspace,
    scheme: SchemeId,
) -> Option<knotq_model::FolderId> {
    workspace
        .folders
        .values()
        .find(|f| f.children.contains(&NodeRef::Scheme(scheme)))
        .map(|f| f.id)
}
