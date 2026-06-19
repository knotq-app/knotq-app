use gpui::ClipboardItem;
use knotq_model::Item;
use serde::{Deserialize, Serialize};

const KNOTQ_CLIPBOARD_FORMAT: &str = "knotq.scheme_items.v1";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct SchemeClipboardPayload {
    pub(super) format: String,
    pub(super) items: Vec<Item>,
    #[serde(default)]
    pub(super) object_selection: bool,
    /// The first/last items are partial line fragments (a selection that spanned
    /// text and a block). Paste splices them into the caret line so a cut
    /// immediately followed by a paste restores the original.
    #[serde(default)]
    pub(super) splice: bool,
}

impl SchemeClipboardPayload {
    pub(super) fn new(items: Vec<Item>) -> Self {
        Self {
            format: KNOTQ_CLIPBOARD_FORMAT.to_string(),
            items,
            object_selection: false,
            splice: false,
        }
    }

    pub(super) fn new_object_selection(items: Vec<Item>) -> Self {
        Self {
            format: KNOTQ_CLIPBOARD_FORMAT.to_string(),
            items,
            object_selection: true,
            splice: false,
        }
    }

    pub(super) fn new_spliced(items: Vec<Item>) -> Self {
        Self {
            format: KNOTQ_CLIPBOARD_FORMAT.to_string(),
            items,
            object_selection: false,
            splice: true,
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
