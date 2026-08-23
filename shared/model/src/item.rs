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
    /// Which glyphs this line's bullet or number is drawn from.
    ///
    /// Separate from [`ItemMarker`] because the marker decides *behaviour* — a
    /// checkbox can be completed, a number participates in an ordinal sequence —
    /// while the family only decides how it looks. Serialized into the marker
    /// string as `bullet.disc` / `numbered.roman` on disk, which a build that
    /// predates families reads as plain `bullet` / `numbered` (see
    /// `marker_base`), so an old build shows the default glyph rather than
    /// failing to open the file.
    #[serde(default, skip_serializing_if = "MarkerFamily::is_standard")]
    pub marker_family: MarkerFamily,
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
            marker_family: MarkerFamily::Standard,
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

    /// The marker as it is written to disk, carrying the family as a suffix
    /// (`bullet.disc`). A build that predates families parses this back to the
    /// base marker, so the line still opens — it just shows the default glyph.
    pub fn marker_token(&self) -> String {
        match self
            .marker_family
            .is_valid_for(self.marker)
            .then(|| self.marker_family.as_suffix())
            .flatten()
        {
            Some(suffix) => format!("{}.{}", self.marker.as_str(), suffix),
            None => self.marker.as_str().to_string(),
        }
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

/// The concrete thing drawn for a marker at one indent depth.
///
/// Separate from [`MarkerFamily`] because a family is a *sequence*: it says
/// which glyph appears at depth 0, depth 1, and so on. This is one entry in
/// that sequence.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum MarkerGlyph {
    Disc,
    Circle,
    Square,
    Dash,
    Decimal,
    LowerAlpha,
    UpperAlpha,
    LowerRoman,
    UpperRoman,
}

impl MarkerGlyph {
    /// Render `ordinal` (1-based) for a numbered glyph. Bullet glyphs have no
    /// ordinal and fall back to the plain number, which is never drawn.
    pub fn ordinal_label(self, ordinal: usize) -> String {
        match self {
            Self::LowerAlpha => alphabetic_ordinal(ordinal, false),
            Self::UpperAlpha => alphabetic_ordinal(ordinal, true),
            Self::LowerRoman => roman_ordinal(ordinal, false),
            Self::UpperRoman => roman_ordinal(ordinal, true),
            _ => ordinal.to_string(),
        }
    }
}

/// A named sequence of marker glyphs, indexed by indent depth.
///
/// EVERY family varies with depth — that is the point of a family. `Standard`
/// is the historical look (disc, then hollow circle, then square as you nest);
/// the others walk a different sequence, so nesting stays legible whichever one
/// you pick. A family that shows the same glyph at every depth is expressed as
/// a one-entry sequence rather than as a special case.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MarkerFamily {
    /// The default look: what every existing line already has.
    #[default]
    Standard,
    // ── bullet families ────────────────────────────────────────────────
    /// A filled disc at every depth.
    Discs,
    /// A hollow ring at every depth.
    Rings,
    /// A small square at every depth.
    Squares,
    /// A dash at every depth — the plainest option, closest to plain text.
    Dashes,
    /// Alternates filled and hollow, which separates adjacent levels more
    /// strongly than a size change does.
    Alternating,
    // ── numbered families ──────────────────────────────────────────────
    /// `1.` at every depth.
    Decimal,
    /// Lower-case letters at every depth.
    Alpha,
    /// Lower-case roman numerals at every depth.
    Roman,
    /// The classic outline sequence: `I.` `A.` `1.` `a.` `i.`
    Outline,
}

impl MarkerFamily {
    pub fn is_standard(&self) -> bool {
        matches!(self, Self::Standard)
    }

    /// The suffix used in the serialized marker string (`bullet.rings`).
    pub const fn as_suffix(self) -> Option<&'static str> {
        match self {
            Self::Standard => None,
            Self::Discs => Some("discs"),
            Self::Rings => Some("rings"),
            Self::Squares => Some("squares"),
            Self::Dashes => Some("dashes"),
            Self::Alternating => Some("alternating"),
            Self::Decimal => Some("decimal"),
            Self::Alpha => Some("alpha"),
            Self::Roman => Some("roman"),
            Self::Outline => Some("outline"),
        }
    }

    /// Parse a marker suffix. An unknown suffix is `Standard` rather than an
    /// error: a NEWER build may write a family this one has never heard of, and
    /// falling back to the default look shows something sensible instead of
    /// refusing the line.
    pub fn from_suffix(suffix: &str) -> Self {
        match suffix {
            "discs" => Self::Discs,
            "rings" => Self::Rings,
            "squares" => Self::Squares,
            "dashes" => Self::Dashes,
            "alternating" => Self::Alternating,
            "decimal" => Self::Decimal,
            "alpha" => Self::Alpha,
            "roman" => Self::Roman,
            "outline" => Self::Outline,
            _ => Self::Standard,
        }
    }

    /// The families offered for `marker`, in the order a picker shows them.
    pub fn choices_for(marker: ItemMarker) -> &'static [MarkerFamily] {
        match marker {
            ItemMarker::Bullet => &[
                Self::Standard,
                Self::Discs,
                Self::Rings,
                Self::Squares,
                Self::Dashes,
                Self::Alternating,
            ],
            ItemMarker::Numbered => &[
                Self::Standard,
                Self::Decimal,
                Self::Alpha,
                Self::Roman,
                Self::Outline,
            ],
            ItemMarker::Blank | ItemMarker::Checkbox => &[],
        }
    }

    pub fn is_valid_for(self, marker: ItemMarker) -> bool {
        self.is_standard() || Self::choices_for(marker).contains(&self)
    }

    /// The glyph this family shows at `depth`.
    ///
    /// Depth wraps around the sequence, so nesting deeper than the sequence is
    /// long keeps cycling rather than running out of glyphs.
    pub fn glyph_at(self, marker: ItemMarker, depth: u8) -> MarkerGlyph {
        let sequence: &[MarkerGlyph] = match (marker, self) {
            (ItemMarker::Numbered, Self::Standard) => &[
                MarkerGlyph::Decimal,
                MarkerGlyph::LowerAlpha,
                MarkerGlyph::LowerRoman,
            ],
            (ItemMarker::Numbered, Self::Decimal) => &[MarkerGlyph::Decimal],
            (ItemMarker::Numbered, Self::Alpha) => &[MarkerGlyph::LowerAlpha],
            (ItemMarker::Numbered, Self::Roman) => &[MarkerGlyph::LowerRoman],
            (ItemMarker::Numbered, Self::Outline) => &[
                MarkerGlyph::UpperRoman,
                MarkerGlyph::UpperAlpha,
                MarkerGlyph::Decimal,
                MarkerGlyph::LowerAlpha,
                MarkerGlyph::LowerRoman,
            ],
            // A numbered line given a bullet family (or vice versa) falls back
            // to that marker's standard sequence rather than drawing nothing.
            (ItemMarker::Numbered, _) => &[
                MarkerGlyph::Decimal,
                MarkerGlyph::LowerAlpha,
                MarkerGlyph::LowerRoman,
            ],
            (_, Self::Discs) => &[MarkerGlyph::Disc],
            (_, Self::Rings) => &[MarkerGlyph::Circle],
            (_, Self::Squares) => &[MarkerGlyph::Square],
            (_, Self::Dashes) => &[MarkerGlyph::Dash],
            (_, Self::Alternating) => &[MarkerGlyph::Disc, MarkerGlyph::Circle],
            // Bullet standard, and any numbered family on a bullet line.
            _ => &[MarkerGlyph::Disc, MarkerGlyph::Circle, MarkerGlyph::Square],
        };
        sequence[depth as usize % sequence.len()]
    }
}


/// 1 -> a, 26 -> z, 27 -> aa, in the spreadsheet-column style. Zero is not a
/// valid ordinal, so it falls back to the number rather than an empty label.
fn alphabetic_ordinal(ordinal: usize, upper: bool) -> String {
    if ordinal == 0 {
        return "0".to_string();
    }
    let base = if upper { b'A' } else { b'a' };
    let mut n = ordinal;
    let mut out = Vec::new();
    while n > 0 {
        let rem = (n - 1) % 26;
        out.push(base + rem as u8);
        n = (n - 1) / 26;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_else(|_| ordinal.to_string())
}

/// Roman numerals. Beyond the classical range the numeral would be absurdly
/// long, so past 3,999 it falls back to the decimal form rather than emitting
/// a wall of `M`s.
fn roman_ordinal(ordinal: usize, upper: bool) -> String {
    if ordinal == 0 || ordinal > 3_999 {
        return ordinal.to_string();
    }
    const TABLE: [(usize, &str); 13] = [
        (1000, "m"), (900, "cm"), (500, "d"), (400, "cd"), (100, "c"),
        (90, "xc"), (50, "l"), (40, "xl"), (10, "x"), (9, "ix"),
        (5, "v"), (4, "iv"), (1, "i"),
    ];
    let mut n = ordinal;
    let mut out = String::new();
    for (value, numeral) in TABLE {
        while n >= value {
            out.push_str(numeral);
            n -= value;
        }
    }
    if upper { out.to_uppercase() } else { out }
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

#[cfg(test)]
mod marker_family_tests {
    use super::*;

    /// The point of a family: it is a SEQUENCE indexed by depth, not one glyph.
    /// The standard bullet sequence is the historical look.
    #[test]
    fn the_standard_family_walks_a_sequence_by_depth() {
        let b = ItemMarker::Bullet;
        assert_eq!(MarkerFamily::Standard.glyph_at(b, 0), MarkerGlyph::Disc);
        assert_eq!(MarkerFamily::Standard.glyph_at(b, 1), MarkerGlyph::Circle);
        assert_eq!(MarkerFamily::Standard.glyph_at(b, 2), MarkerGlyph::Square);
        // Wraps rather than running out.
        assert_eq!(MarkerFamily::Standard.glyph_at(b, 3), MarkerGlyph::Disc);
    }

    /// Every family is depth-indexed — a different family walks a DIFFERENT
    /// sequence over the same depths, which is what distinguishes them.
    #[test]
    fn different_families_differ_at_the_same_depth() {
        let n = ItemMarker::Numbered;
        let depths = [0u8, 1, 2];
        let standard: Vec<_> = depths
            .iter()
            .map(|d| MarkerFamily::Standard.glyph_at(n, *d))
            .collect();
        let outline: Vec<_> = depths
            .iter()
            .map(|d| MarkerFamily::Outline.glyph_at(n, *d))
            .collect();
        assert_eq!(
            standard,
            vec![
                MarkerGlyph::Decimal,
                MarkerGlyph::LowerAlpha,
                MarkerGlyph::LowerRoman
            ]
        );
        assert_eq!(
            outline,
            vec![
                MarkerGlyph::UpperRoman,
                MarkerGlyph::UpperAlpha,
                MarkerGlyph::Decimal
            ]
        );
        assert_ne!(standard, outline);
    }

    /// A single-glyph family is just a one-entry sequence: same glyph at every
    /// depth, no special case in the code.
    #[test]
    fn a_single_glyph_family_is_constant_across_depths() {
        for depth in 0..6 {
            assert_eq!(
                MarkerFamily::Rings.glyph_at(ItemMarker::Bullet, depth),
                MarkerGlyph::Circle
            );
            assert_eq!(
                MarkerFamily::Roman.glyph_at(ItemMarker::Numbered, depth),
                MarkerGlyph::LowerRoman
            );
        }
    }

    /// Alternating exists to separate adjacent levels more strongly than a size
    /// change does, so it must actually alternate.
    #[test]
    fn alternating_flips_between_adjacent_depths() {
        let b = ItemMarker::Bullet;
        assert_eq!(MarkerFamily::Alternating.glyph_at(b, 0), MarkerGlyph::Disc);
        assert_eq!(MarkerFamily::Alternating.glyph_at(b, 1), MarkerGlyph::Circle);
        assert_eq!(MarkerFamily::Alternating.glyph_at(b, 2), MarkerGlyph::Disc);
    }

    #[test]
    fn ordinals_render_in_their_glyph() {
        assert_eq!(MarkerGlyph::Decimal.ordinal_label(7), "7");
        assert_eq!(MarkerGlyph::LowerAlpha.ordinal_label(1), "a");
        assert_eq!(MarkerGlyph::LowerAlpha.ordinal_label(26), "z");
        assert_eq!(MarkerGlyph::LowerAlpha.ordinal_label(27), "aa");
        assert_eq!(MarkerGlyph::UpperAlpha.ordinal_label(28), "AB");
        assert_eq!(MarkerGlyph::LowerRoman.ordinal_label(4), "iv");
        assert_eq!(MarkerGlyph::UpperRoman.ordinal_label(2026), "MMXXVI");
    }

    /// A numeral nobody could read is worse than a number.
    #[test]
    fn absurd_ordinals_fall_back_to_digits() {
        assert_eq!(MarkerGlyph::UpperRoman.ordinal_label(4_000), "4000");
        assert_eq!(MarkerGlyph::LowerRoman.ordinal_label(0), "0");
        assert_eq!(MarkerGlyph::LowerAlpha.ordinal_label(0), "0");
    }

    /// A bullet line must never be left with nothing to draw, even if it somehow
    /// carries a numbered family.
    #[test]
    fn a_mismatched_family_still_draws_something_sensible() {
        assert_eq!(
            MarkerFamily::Roman.glyph_at(ItemMarker::Bullet, 0),
            MarkerGlyph::Disc
        );
        assert_eq!(
            MarkerFamily::Rings.glyph_at(ItemMarker::Numbered, 0),
            MarkerGlyph::Decimal
        );
    }

    #[test]
    fn families_are_scoped_to_their_marker() {
        assert!(MarkerFamily::Squares.is_valid_for(ItemMarker::Bullet));
        assert!(!MarkerFamily::Squares.is_valid_for(ItemMarker::Numbered));
        assert!(MarkerFamily::Roman.is_valid_for(ItemMarker::Numbered));
        assert!(!MarkerFamily::Roman.is_valid_for(ItemMarker::Bullet));
        assert!(MarkerFamily::Standard.is_valid_for(ItemMarker::Checkbox));
    }

    /// A family that could be written but not read back would silently reset on
    /// the next load.
    #[test]
    fn every_family_round_trips_through_its_suffix() {
        for marker in [ItemMarker::Bullet, ItemMarker::Numbered] {
            for family in MarkerFamily::choices_for(marker) {
                match family.as_suffix() {
                    None => assert!(family.is_standard()),
                    Some(suffix) => assert_eq!(
                        MarkerFamily::from_suffix(suffix),
                        *family,
                        "family {family:?} did not survive its suffix {suffix:?}"
                    ),
                }
            }
        }
    }

    /// A family written by a NEWER build must not break this one.
    #[test]
    fn an_unknown_family_falls_back_to_standard() {
        assert_eq!(
            MarkerFamily::from_suffix("holographic"),
            MarkerFamily::Standard
        );
        assert_eq!(MarkerFamily::from_suffix(""), MarkerFamily::Standard);
    }

    /// A workspace that never uses families must serialize byte-identically to
    /// one written before families existed.
    #[test]
    fn the_default_family_is_not_serialized() {
        let plain = Item::new("hello");
        let json = serde_json::to_string(&plain).unwrap();
        assert!(
            !json.contains("marker_family"),
            "default family leaked into output: {json}"
        );

        let mut fancy = Item::new("hello");
        fancy.marker = ItemMarker::Bullet;
        fancy.marker_family = MarkerFamily::Squares;
        let json = serde_json::to_string(&fancy).unwrap();
        assert!(json.contains("squares"), "family missing from output: {json}");
    }

    #[test]
    fn items_without_a_family_load_as_standard() {
        let old = r#"{"id":"00000000-0000-4000-8000-000000000001","marker":"bullet"}"#;
        let item: Item = serde_json::from_str(old).unwrap();
        assert_eq!(item.marker, ItemMarker::Bullet);
        assert_eq!(item.marker_family, MarkerFamily::Standard);
    }
}
