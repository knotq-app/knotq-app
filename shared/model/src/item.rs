use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    CalendarProvider, ItemKind, ItemState, OccurrenceId, OccurrenceState, Recurrence, Table,
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Item {
    #[serde(default)]
    pub id: crate::ItemId,
    /// The line's content as an ordered run of inline pieces: text, inline
    /// images, and tables. Replaces the old `text` + `media` split — a line is
    /// one content stream, so images and tables live *in* the text flow.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub content: Vec<Inline>,
    #[serde(default, skip_serializing_if = "is_default_marker")]
    pub marker: ItemMarker,
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    pub indent: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeats: Option<Recurrence>,
    /// One state slot per stable occurrence identity. Always at least one slot.
    #[serde(default = "default_state", skip_serializing_if = "is_default_state")]
    pub state: Vec<OccurrenceState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external: Option<ExternalItemSource>,
}

/// One piece of a line's content. Most lines are a single [`Inline::Text`]; an
/// image or table is just another inline that flows with the text.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Inline {
    Text { text: String },
    Image(ImageInline),
    Table(Table),
}

impl Inline {
    pub fn text(text: impl Into<String>) -> Self {
        Inline::Text { text: text.into() }
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            Inline::Text { text } => Some(text.as_str()),
            _ => None,
        }
    }

    pub fn is_text(&self) -> bool {
        matches!(self, Inline::Text { .. })
    }
}

/// An inline image. Pixels live on disk under `assets/images/{asset}.{ext}`;
/// the inline only references the asset and remembers its intrinsic size.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ImageInline {
    pub asset: Uuid,
    pub format: ImageAssetFormat,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
}

impl Item {
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            id: crate::ItemId::new(),
            content: if text.is_empty() {
                Vec::new()
            } else {
                vec![Inline::Text { text }]
            },
            marker: ItemMarker::Blank,
            indent: 0,
            start: None,
            end: None,
            available: None,
            repeats: None,
            state: vec![OccurrenceState::default()],
            priority: None,
            external: None,
        }
    }

    pub fn with_indent(mut self, indent: u8) -> Self {
        self.indent = indent;
        self
    }

    pub fn with_start(mut self, dt: DateTime<Utc>) -> Self {
        self.marker = ItemMarker::Checkbox;
        self.start = Some(dt);
        self
    }

    pub fn with_end(mut self, dt: DateTime<Utc>) -> Self {
        self.marker = ItemMarker::Checkbox;
        self.end = Some(dt);
        self
    }

    pub fn with_marker(mut self, marker: ItemMarker) -> Self {
        self.marker = marker;
        self
    }

    pub fn with_repeats(mut self, repeats: Recurrence) -> Self {
        self.marker = ItemMarker::Checkbox;
        self.repeats = Some(repeats);
        self
    }

    pub fn done(mut self) -> Self {
        self.marker = ItemMarker::Checkbox;
        for s in self.state.iter_mut() {
            s.state.progress = -1;
        }
        self
    }

    // ── Content accessors ───────────────────────────────────────────────────

    /// The line's plain text: every [`Inline::Text`] run concatenated. Images
    /// and tables contribute nothing. This is the analog of the old `text`
    /// field and the value most non-editor consumers want.
    pub fn text(&self) -> String {
        let mut out = String::new();
        for inline in &self.content {
            if let Inline::Text { text } = inline {
                out.push_str(text);
            }
        }
        out
    }

    /// True when the line has no text and no images/tables.
    pub fn is_content_empty(&self) -> bool {
        self.content.iter().all(|inline| match inline {
            Inline::Text { text } => text.is_empty(),
            _ => false,
        })
    }

    /// Replace the line's text with a single run, preserving any images/tables
    /// (kept after the text, matching the old "text and media are independent"
    /// behavior). Callers doing fine-grained inline editing manipulate
    /// `content` directly instead.
    pub fn set_text(&mut self, text: impl Into<String>) {
        let text = text.into();
        let mut rest: Vec<Inline> = std::mem::take(&mut self.content)
            .into_iter()
            .filter(|inline| !inline.is_text())
            .collect();
        if text.is_empty() {
            self.content = rest;
        } else {
            self.content = Vec::with_capacity(rest.len() + 1);
            self.content.push(Inline::Text { text });
            self.content.append(&mut rest);
        }
    }

    /// Iterator over the inline images on this line, in order.
    pub fn images(&self) -> impl Iterator<Item = &ImageInline> {
        self.content.iter().filter_map(|inline| match inline {
            Inline::Image(image) => Some(image),
            _ => None,
        })
    }

    pub fn has_images(&self) -> bool {
        self.content.iter().any(|i| matches!(i, Inline::Image(_)))
    }

    /// The first table on this line, if any.
    pub fn table(&self) -> Option<&Table> {
        self.content.iter().find_map(|inline| match inline {
            Inline::Table(table) => Some(table),
            _ => None,
        })
    }

    pub fn table_mut(&mut self) -> Option<&mut Table> {
        self.content.iter_mut().find_map(|inline| match inline {
            Inline::Table(table) => Some(table),
            _ => None,
        })
    }

    pub fn has_table(&self) -> bool {
        self.content.iter().any(|i| matches!(i, Inline::Table(_)))
    }

    /// Append an inline image to the end of the content.
    pub fn push_image(&mut self, image: ImageInline) {
        self.content.push(Inline::Image(image));
    }

    pub fn kind(&self) -> ItemKind {
        if self.marker != ItemMarker::Checkbox {
            return ItemKind::Procedure;
        }
        match (self.start.is_some(), self.end.is_some()) {
            (true, true) => ItemKind::Event,
            (true, false) => ItemKind::Reminder,
            (false, true) => ItemKind::Assignment,
            (false, false) => ItemKind::Procedure,
        }
    }

    pub fn state_for_occurrence(&self, occurrence: &OccurrenceId) -> ItemState {
        self.state
            .iter()
            .find(|state| &state.occurrence == occurrence)
            .map(|state| state.state)
            .unwrap_or_default()
    }

    pub fn state_for_occurrence_mut(&mut self, occurrence: OccurrenceId) -> &mut ItemState {
        if let Some(index) = self
            .state
            .iter()
            .position(|state| state.occurrence == occurrence)
        {
            return &mut self.state[index].state;
        }
        self.state.push(OccurrenceState {
            occurrence,
            state: ItemState::default(),
        });
        &mut self.state.last_mut().unwrap().state
    }

    pub fn single_state(&self) -> ItemState {
        self.state_for_occurrence(&OccurrenceId::Single)
    }

    pub fn normalize_state(&mut self) {
        self.state
            .retain(|state| state.occurrence == OccurrenceId::Single || !state.state.is_default());
        if self.state.is_empty() {
            self.state.push(OccurrenceState::default());
        }
    }

    pub fn enforce_marker_constraints(&mut self) -> bool {
        let mut changed = false;
        if self.marker == ItemMarker::Checkbox {
            if self.state.is_empty() {
                self.state.push(OccurrenceState::default());
                changed = true;
            }
            self.normalize_state();
            return changed;
        }

        if self.start.take().is_some() {
            changed = true;
        }
        if self.end.take().is_some() {
            changed = true;
        }
        if self.available.take().is_some() {
            changed = true;
        }
        if self.repeats.take().is_some() {
            changed = true;
        }
        let state_has_annotations = self.state.len() != 1
            || self.state.first().is_none_or(|state| {
                state.occurrence != OccurrenceId::Single || !state.state.is_default()
            });
        if state_has_annotations {
            self.state = vec![OccurrenceState::default()];
            changed = true;
        }
        changed
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExternalItemSource {
    pub provider: CalendarProvider,
    pub account_id: String,
    pub calendar_id: String,
    pub event_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemMarker {
    #[default]
    Blank,
    Bullet,
    Numbered,
    Checkbox,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageAssetFormat {
    Png,
    Jpeg,
    Webp,
    Gif,
    Svg,
    Bmp,
    Tiff,
}

impl ImageAssetFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Webp => "webp",
            Self::Gif => "gif",
            Self::Svg => "svg",
            Self::Bmp => "bmp",
            Self::Tiff => "tiff",
        }
    }
}

fn default_state() -> Vec<OccurrenceState> {
    vec![OccurrenceState::default()]
}

fn is_zero_u8(value: &u8) -> bool {
    *value == 0
}

fn is_default_marker(marker: &ItemMarker) -> bool {
    *marker == ItemMarker::Blank
}

fn is_default_state(state: &[OccurrenceState]) -> bool {
    state.len() == 1 && state[0].occurrence == OccurrenceId::Single && state[0].state.is_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn image() -> ImageInline {
        ImageInline {
            asset: Uuid::new_v4(),
            format: ImageAssetFormat::Png,
            width: Some(10),
            height: Some(20),
        }
    }

    #[test]
    fn new_empty_text_has_no_content() {
        let item = Item::new("");
        assert!(item.content.is_empty());
        assert_eq!(item.text(), "");
        assert!(item.is_content_empty());
    }

    #[test]
    fn text_concatenates_text_runs_only() {
        let item = Item {
            content: vec![Inline::text("a"), Inline::Image(image()), Inline::text("b")],
            ..Item::new("")
        };
        assert_eq!(item.text(), "ab");
        assert!(!item.is_content_empty());
        assert_eq!(item.images().count(), 1);
        assert!(item.has_images());
    }

    #[test]
    fn set_text_replaces_text_but_keeps_images() {
        let mut item = Item::new("hello");
        let img = image();
        item.push_image(img);
        item.set_text("world");
        assert_eq!(item.text(), "world");
        // The image survives and now follows the new text run.
        assert_eq!(
            item.content,
            vec![Inline::text("world"), Inline::Image(img)]
        );
    }

    #[test]
    fn set_text_empty_drops_text_run_keeps_images() {
        let mut item = Item::new("hello");
        item.push_image(image());
        item.set_text("");
        assert_eq!(item.text(), "");
        assert_eq!(item.images().count(), 1);
    }

    #[test]
    fn inline_serde_roundtrips() {
        let inlines = vec![Inline::text("hi"), Inline::Image(image())];
        let json = serde_json::to_string(&inlines).unwrap();
        let back: Vec<Inline> = serde_json::from_str(&json).unwrap();
        assert_eq!(inlines, back);
    }
}
