use gpui::ClipboardItem;
use knotq_model::Item;
use serde::{Deserialize, Serialize};

const KNOTQ_CLIPBOARD_FORMAT: &str = "knotq.scheme_items.v1";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct SchemeClipboardPayload {
    pub(super) format: String,
    pub(super) items: Vec<Item>,
}

impl SchemeClipboardPayload {
    pub(super) fn new(items: Vec<Item>) -> Self {
        Self {
            format: KNOTQ_CLIPBOARD_FORMAT.to_string(),
            items,
        }
    }
}

pub(super) fn rich_clipboard_payload(item: &ClipboardItem) -> Option<SchemeClipboardPayload> {
    let payload: SchemeClipboardPayload = serde_json::from_str(item.metadata()?).ok()?;
    if payload.format == KNOTQ_CLIPBOARD_FORMAT && !payload.items.is_empty() {
        Some(payload)
    } else {
        None
    }
}
