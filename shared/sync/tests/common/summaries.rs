//! Byte-comparable workspace summaries used by the convergence checks.
use super::*;

#[derive(Debug, Eq, PartialEq)]
pub(super) struct WorkspaceSummary {
    pub(super) workspace_document: String,
    pub(super) root_schemes: Vec<String>,
    pub(super) folders: Vec<FolderSummary>,
    pub(super) recently_deleted: Vec<String>,
    pub(super) daily_queue: Vec<(String, String)>,
    pub(super) schemes: Vec<SchemeSummary>,
}

#[derive(Debug, Eq, PartialEq)]
pub(super) struct FolderSummary {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) children: Vec<String>,
}

#[derive(Debug, Eq, PartialEq)]
pub(super) struct SchemeSummary {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) archived: bool,
    pub(super) gsync: bool,
    pub(super) source: String,
    pub(super) items: Vec<String>,
}

pub(super) fn node_ref_label(node: &NodeRef) -> String {
    match node {
        NodeRef::Folder(id) => format!("folder:{id}"),
        NodeRef::Scheme(id) => format!("scheme:{id}"),
    }
}

pub(super) fn item_summary(item: &Item) -> String {
    serde_json::to_string(item).expect("item should serialize")
}

/// A stable label for a scheme's source so the convergence check catches a lost or
/// diverged imported-calendar association (provider/account/calendar), not just the
/// local-vs-imported distinction.
pub(super) fn scheme_source_label(source: &SchemeSource) -> String {
    match source {
        SchemeSource::Local => "local".to_string(),
        SchemeSource::ImportedCalendar(source) => format!(
            "imported:{:?}:{}:{}:{}:{}",
            source.provider,
            source.account_id,
            source.account_email.as_deref().unwrap_or(""),
            source.calendar_id,
            source.read_only,
        ),
    }
}
