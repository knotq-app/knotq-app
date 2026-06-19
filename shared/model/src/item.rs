use chrono::{DateTime, Utc};
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use uuid::Uuid;

use crate::{
    CalendarProvider, ItemKind, ItemState, OccurrenceId, OccurrenceState, Recurrence, Table,
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Item {
    #[serde(default)]
    pub id: crate::ItemId,
    /// The line's content. A line is *exactly one* of: plain text, a single
    /// image, or a single table. Images and tables are whole-line block objects
    /// — they never share a line with text or with each other. The cursor treats
    /// a block line as an atomic object (select/delete the whole line).
    #[serde(default, skip_serializing_if = "ItemContent::is_empty_text")]
    pub content: ItemContent,
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

/// The whole content of a single line: exactly one of plain text, one image, or
/// one table. This is the model's per-line content type — a line can never mix
/// these, so an image or table always occupies a line by itself.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ItemContent {
    Text { text: String },
    Image(ImageInline),
    Table(Table),
}

impl Default for ItemContent {
    fn default() -> Self {
        ItemContent::Text {
            text: String::new(),
        }
    }
}

impl ItemContent {
    pub fn text(text: impl Into<String>) -> Self {
        ItemContent::Text { text: text.into() }
    }

    /// The text of a text line, or `None` for an image/table line.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            ItemContent::Text { text } => Some(text.as_str()),
            _ => None,
        }
    }

    pub fn is_text(&self) -> bool {
        matches!(self, ItemContent::Text { .. })
    }

    /// True for an image or table line (a whole-line block object).
    pub fn is_block(&self) -> bool {
        matches!(self, ItemContent::Image(_) | ItemContent::Table(_))
    }

    /// True for an empty text line — the default "blank" content.
    pub fn is_empty_text(&self) -> bool {
        matches!(self, ItemContent::Text { text } if text.is_empty())
    }

    pub fn image(&self) -> Option<&ImageInline> {
        match self {
            ItemContent::Image(image) => Some(image),
            _ => None,
        }
    }

    pub fn table(&self) -> Option<&Table> {
        match self {
            ItemContent::Table(table) => Some(table),
            _ => None,
        }
    }

    pub fn table_mut(&mut self) -> Option<&mut Table> {
        match self {
            ItemContent::Table(table) => Some(table),
            _ => None,
        }
    }

    // ── CRDT bridge ─────────────────────────────────────────────────────────
    //
    // The collaborative engine still represents a line as a run of [`Inline`]
    // units (text characters plus image/table embeds) so its character-level
    // merge machinery is unchanged. These convert between that flat run and the
    // model's single-content form at the boundary.

    /// Flatten to the CRDT's inline run. An empty text line is the empty run.
    pub fn to_inlines(&self) -> Vec<Inline> {
        match self {
            ItemContent::Text { text } if text.is_empty() => Vec::new(),
            ItemContent::Text { text } => vec![Inline::Text { text: text.clone() }],
            ItemContent::Image(image) => vec![Inline::Image(*image)],
            ItemContent::Table(table) => vec![Inline::Table(table.clone())],
        }
    }

    /// Collapse a CRDT inline run back to single-content. If a merge ever yields
    /// a mix, the first block (image/table) in document order wins; otherwise the
    /// text runs are concatenated. This keeps convergence deterministic.
    pub fn from_inlines(inlines: Vec<Inline>) -> Self {
        for inline in &inlines {
            match inline {
                Inline::Image(image) => return ItemContent::Image(*image),
                Inline::Table(table) => return ItemContent::Table(table.clone()),
                Inline::Text { .. } => {}
            }
        }
        let mut text = String::new();
        for inline in inlines {
            if let Inline::Text { text: chunk } = inline {
                text.push_str(&chunk);
            }
        }
        ItemContent::Text { text }
    }
}

/// One unit of a line in the collaborative engine: a text run or an image/table
/// embed. This is an *internal* representation used only by the sync CRDT and
/// the `ItemContent` bridge above — the model field is [`ItemContent`], which
/// constrains a line to a single content kind.
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
            content: ItemContent::Text { text },
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

    /// The line's plain text, or the empty string for an image/table line. This
    /// is the value most non-editor consumers want.
    pub fn text(&self) -> String {
        self.content.as_text().unwrap_or("").to_string()
    }

    /// True when the line is an empty text line (no text, no image/table).
    pub fn is_content_empty(&self) -> bool {
        self.content.is_empty_text()
    }

    /// Make this a text line with the given text (replacing any image/table).
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.content = ItemContent::Text { text: text.into() };
    }

    /// Iterator over the line's image (zero or one — a line holds at most one).
    pub fn images(&self) -> impl Iterator<Item = &ImageInline> {
        self.content.image().into_iter()
    }

    pub fn has_images(&self) -> bool {
        matches!(self.content, ItemContent::Image(_))
    }

    /// This line's table, if it is a table line.
    pub fn table(&self) -> Option<&Table> {
        self.content.table()
    }

    pub fn table_mut(&mut self) -> Option<&mut Table> {
        self.content.table_mut()
    }

    pub fn has_table(&self) -> bool {
        matches!(self.content, ItemContent::Table(_))
    }

    /// Make this an image line (replacing any prior text/image/table content).
    pub fn set_image(&mut self, image: ImageInline) {
        self.content = ItemContent::Image(image);
    }

    /// Make this a table line (replacing any prior text/image/table content).
    pub fn set_table(&mut self, table: Table) {
        self.content = ItemContent::Table(table);
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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ItemMarker {
    #[default]
    Blank,
    Bullet,
    Numbered,
    Checkbox,
}

impl ItemMarker {
    pub fn parse(value: &str) -> Result<Self, ParseItemMarkerError> {
        let base = marker_base(value).ok_or_else(|| ParseItemMarkerError {
            value: value.to_string(),
        })?;
        match base {
            "blank" => Ok(Self::Blank),
            "bullet" => Ok(Self::Bullet),
            "numbered" => Ok(Self::Numbered),
            "checkbox" => Ok(Self::Checkbox),
            _ => Err(ParseItemMarkerError {
                value: value.to_string(),
            }),
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Blank => "blank",
            Self::Bullet => "bullet",
            Self::Numbered => "numbered",
            Self::Checkbox => "checkbox",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParseItemMarkerError {
    value: String,
}

impl fmt::Display for ParseItemMarkerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown item marker {:?}", self.value)
    }
}

impl std::error::Error for ParseItemMarkerError {}

impl Serialize for ItemMarker {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ItemMarker {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).map_err(de::Error::custom)
    }
}

fn marker_base(value: &str) -> Option<&str> {
    match value.split_once('.') {
        Some((base, subtype)) if !base.is_empty() && !subtype.is_empty() => Some(base),
        Some(_) => None,
        None if !value.is_empty() => Some(value),
        None => None,
    }
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
        assert!(item.content.is_empty_text());
        assert_eq!(item.text(), "");
        assert!(item.is_content_empty());
    }

    #[test]
    fn text_line_reports_text_and_no_block() {
        let item = Item::new("ab");
        assert_eq!(item.text(), "ab");
        assert!(!item.is_content_empty());
        assert_eq!(item.images().count(), 0);
        assert!(!item.has_images());
        assert!(!item.has_table());
    }

    #[test]
    fn image_line_is_a_block_with_no_text() {
        let mut item = Item::new("");
        item.set_image(image());
        assert_eq!(item.text(), "");
        assert!(!item.is_content_empty());
        assert_eq!(item.images().count(), 1);
        assert!(item.has_images());
        assert!(item.content.is_block());
    }

    #[test]
    fn set_text_replaces_any_block_with_text() {
        let mut item = Item::new("");
        item.set_image(image());
        item.set_text("world");
        assert_eq!(item.text(), "world");
        assert!(!item.has_images());
        assert_eq!(item.content, ItemContent::text("world"));
    }

    #[test]
    fn content_inline_bridge_roundtrips_each_kind() {
        for content in [
            ItemContent::text("hi"),
            ItemContent::text(""),
            ItemContent::Image(image()),
        ] {
            let back = ItemContent::from_inlines(content.to_inlines());
            assert_eq!(content, back);
        }
    }

    #[test]
    fn from_inlines_prefers_block_on_mixed_run() {
        let img = image();
        let mixed = vec![Inline::text("a"), Inline::Image(img), Inline::text("b")];
        assert_eq!(ItemContent::from_inlines(mixed), ItemContent::Image(img));
    }

    #[test]
    fn inline_serde_roundtrips() {
        let inlines = vec![Inline::text("hi"), Inline::Image(image())];
        let json = serde_json::to_string(&inlines).unwrap();
        let back: Vec<Inline> = serde_json::from_str(&json).unwrap();
        assert_eq!(inlines, back);
    }

    #[test]
    fn marker_deserializes_dotted_subtypes_as_base_markers() {
        for (raw, marker) in [
            ("\"blank.legacy\"", ItemMarker::Blank),
            ("\"bullet.disc\"", ItemMarker::Bullet),
            ("\"numbered.alphabet\"", ItemMarker::Numbered),
            ("\"checkbox.square\"", ItemMarker::Checkbox),
        ] {
            let parsed: ItemMarker = serde_json::from_str(raw).unwrap();
            assert_eq!(parsed, marker);
        }
    }

    #[test]
    fn marker_serializes_base_marker_name() {
        assert_eq!(
            serde_json::to_string(&ItemMarker::Numbered).unwrap(),
            "\"numbered\""
        );
    }

    #[test]
    fn marker_rejects_unknown_or_empty_dotted_markers() {
        for raw in [
            "\"list.alphabet\"",
            "\"numbered.\"",
            "\".alphabet\"",
            "\"\"",
        ] {
            assert!(serde_json::from_str::<ItemMarker>(raw).is_err());
        }
    }
}
