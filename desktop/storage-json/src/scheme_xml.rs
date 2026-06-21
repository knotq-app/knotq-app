//! XML on-disk format for scheme files (single-content model).
//!
//! A scheme is a list of `<item>`s. An item's content is *exactly one* of a
//! `<text>`, an `<image>`, or a `<table>` child — a line is one content kind, so
//! images and tables are whole-line blocks. A table cell is just a list of
//! `<item>`s, encoded exactly like a top-level line, so the document is one
//! uniform tree.
//!
//! Single-valued fields are attributes; the rich nested fields (recurrence
//! overrides, state, external source) ride as JSON inside child elements,
//! reusing the model's serde — the same split the markdown format used, so those
//! representations stay battle-tested.
//!
//! The encoder is written by hand (full control of escaping); the decoder uses
//! `quick-xml`'s pull parser for correct entity/attribute handling.

use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use knotq_model::{
    ExternalItemSource, ImageAssetFormat, ImageInline, Inline, Item, ItemContent, ItemId,
    ItemMarker, OccurrenceId, OccurrenceState, Recurrence, Scheme, SchemeId, Table, TableCell,
    TableColumn, TableRow,
};
use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event};
use quick_xml::reader::Reader;
use quick_xml::writer::Writer;
use std::borrow::Cow;
use std::io::Write;
use std::path::Path;

use crate::scheme_file::SchemeFile;

const SCHEME_VERSION: &str = "1";

// ── Encoding ──────────────────────────────────────────────────────────────

pub(crate) fn encode_scheme_xml(scheme: &Scheme) -> Result<String> {
    // The encoder writes through quick-xml's `Writer`, so all element, attribute,
    // and text escaping is the library's responsibility — there is no hand-rolled
    // escaping that a newly added field could forget, which structurally rules out
    // XML injection. We only pre-strip characters illegal in XML 1.0, which no
    // escaping can represent (see `strip_invalid_xml_chars`).
    let mut writer = Writer::new_with_indent(Vec::new(), b' ', 2);
    writer.write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))?;
    let mut scheme_el = BytesStart::new("scheme");
    scheme_el.push_attribute(("id", scheme.id.to_string().as_str()));
    scheme_el.push_attribute(("version", SCHEME_VERSION));
    writer.write_event(Event::Start(scheme_el))?;
    for item in &scheme.items {
        write_item(&mut writer, item)?;
    }
    writer.write_event(Event::End(BytesEnd::new("scheme")))?;
    let mut bytes = writer.into_inner();
    bytes.push(b'\n');
    Ok(String::from_utf8(bytes)?)
}

fn write_item<W: Write>(writer: &mut Writer<W>, item: &Item) -> Result<()> {
    let mut el = BytesStart::new("item");
    push_item_attrs(&mut el, item);
    let meta = meta_children(item)?;

    match &item.content {
        ItemContent::Table(table) => {
            writer.write_event(Event::Start(el))?;
            write_table(writer, table)?;
            write_meta(writer, &meta)?;
            writer.write_event(Event::End(BytesEnd::new("item")))?;
        }
        ItemContent::Image(image) => {
            writer.write_event(Event::Start(el))?;
            write_image(writer, image)?;
            write_meta(writer, &meta)?;
            writer.write_event(Event::End(BytesEnd::new("item")))?;
        }
        ItemContent::Text { text } if !text.is_empty() => {
            writer.write_event(Event::Start(el))?;
            write_text_element(writer, "text", text)?;
            write_meta(writer, &meta)?;
            writer.write_event(Event::End(BytesEnd::new("item")))?;
        }
        // Empty text content: a self-closing item when there is no metadata,
        // otherwise an open item carrying just its metadata children.
        ItemContent::Text { .. } => {
            if meta.is_empty() {
                writer.write_event(Event::Empty(el))?;
            } else {
                writer.write_event(Event::Start(el))?;
                write_meta(writer, &meta)?;
                writer.write_event(Event::End(BytesEnd::new("item")))?;
            }
        }
    }
    Ok(())
}

/// Item metadata children (complex recurrence, non-default state, external
/// source) ride as JSON text inside a named element, reusing the model's serde.
/// Returned as `(tag, json)` pairs for the writer to emit and escape.
fn meta_children(item: &Item) -> Result<Vec<(&'static str, String)>> {
    let mut children = Vec::new();
    if let Some(repeats) = &item.repeats {
        if single_rrule(repeats).is_none() {
            children.push(("repeats", serde_json::to_string(repeats)?));
        }
    }
    if !state_is_default(&item.state) {
        children.push(("state", serde_json::to_string(&item.state)?));
    }
    if let Some(external) = &item.external {
        children.push(("external", serde_json::to_string(external)?));
    }
    Ok(children)
}

fn write_meta<W: Write>(writer: &mut Writer<W>, meta: &[(&'static str, String)]) -> Result<()> {
    for (tag, json) in meta {
        write_text_element(writer, tag, json)?;
    }
    Ok(())
}

/// Write `<tag>text</tag>`; the library escapes the content. We only strip
/// characters illegal in XML 1.0 first.
fn write_text_element<W: Write>(writer: &mut Writer<W>, tag: &str, text: &str) -> Result<()> {
    writer.write_event(Event::Start(BytesStart::new(tag)))?;
    writer.write_event(Event::Text(BytesText::new(&strip_invalid_xml_chars(text))))?;
    writer.write_event(Event::End(BytesEnd::new(tag)))?;
    Ok(())
}

fn write_image<W: Write>(writer: &mut Writer<W>, image: &ImageInline) -> Result<()> {
    let mut el = BytesStart::new("image");
    el.push_attribute(("asset", image.asset.to_string().as_str()));
    el.push_attribute(("format", image_format_str(image.format)));
    if let Some(width) = image.width {
        el.push_attribute(("width", width.to_string().as_str()));
    }
    if let Some(height) = image.height {
        el.push_attribute(("height", height.to_string().as_str()));
    }
    writer.write_event(Event::Empty(el))?;
    Ok(())
}

fn write_table<W: Write>(writer: &mut Writer<W>, table: &Table) -> Result<()> {
    writer.write_event(Event::Start(BytesStart::new("table")))?;
    for column in &table.columns {
        let mut el = BytesStart::new("column");
        el.push_attribute(("id", column.id.to_string().as_str()));
        el.push_attribute(("name", strip_invalid_xml_chars(&column.name).as_ref()));
        if let Some(width) = column.width {
            el.push_attribute(("width", width.to_string().as_str()));
        }
        writer.write_event(Event::Empty(el))?;
    }
    for row in &table.rows {
        let mut row_el = BytesStart::new("row");
        row_el.push_attribute(("id", row.id.to_string().as_str()));
        writer.write_event(Event::Start(row_el))?;
        for cell in &row.cells {
            writer.write_event(Event::Start(BytesStart::new("cell")))?;
            for item in &cell.items {
                write_item(writer, item)?;
            }
            writer.write_event(Event::End(BytesEnd::new("cell")))?;
        }
        writer.write_event(Event::End(BytesEnd::new("row")))?;
    }
    writer.write_event(Event::End(BytesEnd::new("table")))?;
    Ok(())
}

fn push_item_attrs(el: &mut BytesStart, item: &Item) {
    el.push_attribute(("id", item.id.to_string().as_str()));
    el.push_attribute(("marker", item.marker.as_str()));
    if item.indent != 0 {
        el.push_attribute(("indent", item.indent.to_string().as_str()));
    }
    if let Some(start) = item.start {
        el.push_attribute(("start", encode_datetime(start).as_str()));
    }
    if let Some(end) = item.end {
        el.push_attribute(("end", encode_datetime(end).as_str()));
    }
    if let Some(available) = item.available {
        el.push_attribute(("available", encode_datetime(available).as_str()));
    }
    if let Some(priority) = item.priority {
        el.push_attribute(("priority", priority.to_string().as_str()));
    }
    if let Some(repeats) = &item.repeats {
        if let Some(rrule) = single_rrule(repeats) {
            el.push_attribute(("rrule", strip_invalid_xml_chars(rrule).as_ref()));
        }
    }
}

fn image_format_str(format: ImageAssetFormat) -> &'static str {
    match format {
        ImageAssetFormat::Png => "png",
        ImageAssetFormat::Jpeg => "jpeg",
        ImageAssetFormat::Webp => "webp",
        ImageAssetFormat::Gif => "gif",
        ImageAssetFormat::Svg => "svg",
        ImageAssetFormat::Bmp => "bmp",
        ImageAssetFormat::Tiff => "tiff",
    }
}

fn parse_image_format(value: &str) -> Result<ImageAssetFormat> {
    Ok(match value {
        "png" => ImageAssetFormat::Png,
        "jpeg" | "jpg" => ImageAssetFormat::Jpeg,
        "webp" => ImageAssetFormat::Webp,
        "gif" => ImageAssetFormat::Gif,
        "svg" => ImageAssetFormat::Svg,
        "bmp" => ImageAssetFormat::Bmp,
        "tiff" | "tif" => ImageAssetFormat::Tiff,
        other => bail!("unknown image format {other:?}"),
    })
}

/// Whether `c` is a legal XML 1.0 character. Control characters (other than tab,
/// newline, and CR) are forbidden and cannot be represented even via numeric
/// escapes, so they must be removed rather than encoded.
fn is_xml_char(c: char) -> bool {
    matches!(c,
        '\u{9}' | '\u{A}' | '\u{D}'
        | '\u{20}'..='\u{D7FF}'
        | '\u{E000}'..='\u{FFFD}'
        | '\u{10000}'..='\u{10FFFF}')
}

/// Drop characters illegal in XML 1.0. Without this, a pasted/imported control
/// character would be written verbatim and then fail to parse on the next load,
/// silently losing the whole scheme. Borrows when the input is already clean.
fn strip_invalid_xml_chars(text: &str) -> Cow<'_, str> {
    if text.chars().all(is_xml_char) {
        Cow::Borrowed(text)
    } else {
        Cow::Owned(text.chars().filter(|&c| is_xml_char(c)).collect())
    }
}


// ── Decoding ──────────────────────────────────────────────────────────────

pub(crate) fn decode_scheme_xml(raw: &str, path: &Path, id: SchemeId) -> Result<SchemeFile> {
    if raw.trim().is_empty() {
        return Ok(SchemeFile {
            id,
            items: Vec::new(),
        });
    }
    let mut reader = Reader::from_str(raw);
    let mut items = Vec::new();
    let ctx = || format!("parse scheme XML in {}", path.display());
    loop {
        match reader.read_event().with_context(ctx)? {
            Event::Start(e) if e.name().as_ref() == b"item" => {
                items.push(read_item(&mut reader, &e, path)?);
            }
            Event::Empty(e) if e.name().as_ref() == b"item" => {
                let mut item = parse_item_attrs(&e).with_context(ctx)?;
                item.enforce_marker_constraints();
                items.push(item);
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(SchemeFile { id, items })
}

/// Read an `<item>` element (already consumed as `start`) through its matching
/// `</item>`, collecting ordered content inlines plus JSON metadata children.
fn read_item(reader: &mut Reader<&[u8]>, start: &BytesStart, path: &Path) -> Result<Item> {
    let mut item = parse_item_attrs(start)
        .with_context(|| format!("parse item attributes in {}", path.display()))?;
    let mut content: Vec<Inline> = Vec::new();
    loop {
        match reader.read_event()? {
            Event::Empty(e) if e.name().as_ref() == b"image" => {
                content.push(Inline::Image(parse_image(&e)?));
            }
            Event::Start(e) => match e.name().as_ref() {
                b"text" => {
                    let text = read_element_text(reader, b"text")?;
                    content.push(Inline::Text { text });
                }
                b"image" => {
                    let image = parse_image(&e)?;
                    skip_to_end(reader, b"image")?;
                    content.push(Inline::Image(image));
                }
                b"table" => content.push(Inline::Table(read_table(reader, path)?)),
                b"repeats" => {
                    let json = read_element_text(reader, b"repeats")?;
                    item.repeats =
                        Some(serde_json::from_str::<Recurrence>(&json).context("parse repeats")?);
                }
                b"state" => {
                    let json = read_element_text(reader, b"state")?;
                    item.state = serde_json::from_str(&json).context("parse state")?;
                }
                b"external" => {
                    let json = read_element_text(reader, b"external")?;
                    item.external = Some(
                        serde_json::from_str::<ExternalItemSource>(&json)
                            .context("parse external")?,
                    );
                }
                _ => {}
            },
            Event::End(e) if e.name().as_ref() == b"item" => break,
            Event::Eof => bail!("unexpected EOF inside <item>"),
            _ => {}
        }
    }
    item.content = ItemContent::from_inlines(content);
    item.enforce_marker_constraints();
    Ok(item)
}

fn read_table(reader: &mut Reader<&[u8]>, path: &Path) -> Result<Table> {
    let mut columns: Vec<TableColumn> = Vec::new();
    let mut rows: Vec<TableRow> = Vec::new();
    loop {
        match reader.read_event()? {
            Event::Empty(e) if e.name().as_ref() == b"column" => {
                columns.push(parse_column(&e)?);
            }
            Event::Start(e) if e.name().as_ref() == b"column" => {
                let column = parse_column(&e)?;
                skip_to_end(reader, b"column")?;
                columns.push(column);
            }
            Event::Start(e) if e.name().as_ref() == b"row" => {
                rows.push(read_row(reader, &e, path)?);
            }
            Event::Empty(e) if e.name().as_ref() == b"row" => {
                rows.push(TableRow {
                    id: parse_row_id(&e),
                    cells: Vec::new(),
                });
            }
            Event::End(e) if e.name().as_ref() == b"table" => break,
            Event::Eof => bail!("unexpected EOF inside <table>"),
            _ => {}
        }
    }
    let mut table = Table { columns, rows };
    table.normalize();
    Ok(table)
}

fn read_row(reader: &mut Reader<&[u8]>, start: &BytesStart, path: &Path) -> Result<TableRow> {
    let id = parse_row_id(start);
    let mut cells = Vec::new();
    loop {
        match reader.read_event()? {
            Event::Start(e) if e.name().as_ref() == b"cell" => {
                cells.push(read_cell(reader, path)?);
            }
            Event::End(e) if e.name().as_ref() == b"row" => break,
            Event::Eof => bail!("unexpected EOF inside <row>"),
            _ => {}
        }
    }
    Ok(TableRow { id, cells })
}

/// Read a `<cell>…</cell>` block: a list of line `<item>`s.
fn read_cell(reader: &mut Reader<&[u8]>, path: &Path) -> Result<TableCell> {
    let mut items = Vec::new();
    loop {
        match reader.read_event()? {
            Event::Start(e) if e.name().as_ref() == b"item" => {
                items.push(read_item(reader, &e, path)?);
            }
            Event::Empty(e) if e.name().as_ref() == b"item" => {
                let mut item = parse_item_attrs(&e)?;
                item.enforce_marker_constraints();
                items.push(item);
            }
            Event::End(e) if e.name().as_ref() == b"cell" => break,
            Event::Eof => bail!("unexpected EOF inside <cell>"),
            _ => {}
        }
    }
    Ok(TableCell::from_items(items))
}

fn parse_item_attrs(e: &BytesStart) -> Result<Item> {
    let mut item = Item::new("");
    for attr in e.attributes() {
        let attr = attr.context("read item attribute")?;
        let value = attr.unescape_value().context("unescape item attribute")?;
        match attr.key.as_ref() {
            b"id" => item.id = value.parse::<ItemId>().context("parse item id")?,
            b"marker" => item.marker = parse_marker(&value)?,
            b"indent" => item.indent = value.parse::<u8>().context("parse indent")?,
            b"start" => item.start = Some(parse_datetime(&value).context("parse start")?),
            b"end" => item.end = Some(parse_datetime(&value).context("parse end")?),
            b"available" => {
                item.available = Some(parse_datetime(&value).context("parse available")?)
            }
            b"priority" => item.priority = Some(value.parse::<u8>().context("parse priority")?),
            b"rrule" => {
                item.repeats = Some(Recurrence {
                    rrules: vec![value.to_string()],
                    ..Default::default()
                })
            }
            _ => {}
        }
    }
    Ok(item)
}

fn parse_image(e: &BytesStart) -> Result<ImageInline> {
    let mut asset = None;
    let mut format = None;
    let mut width = None;
    let mut height = None;
    for attr in e.attributes() {
        let attr = attr.context("read image attribute")?;
        let value = attr.unescape_value().context("unescape image attribute")?;
        match attr.key.as_ref() {
            b"asset" => asset = Some(value.parse().context("parse image asset")?),
            b"format" => format = Some(parse_image_format(&value)?),
            b"width" => width = value.parse::<u32>().ok(),
            b"height" => height = value.parse::<u32>().ok(),
            _ => {}
        }
    }
    Ok(ImageInline {
        asset: asset.ok_or_else(|| anyhow!("image missing asset"))?,
        format: format.ok_or_else(|| anyhow!("image missing format"))?,
        width,
        height,
    })
}

fn parse_column(e: &BytesStart) -> Result<TableColumn> {
    let mut column = TableColumn::new("");
    for attr in e.attributes() {
        let attr = attr.context("read column attribute")?;
        let value = attr.unescape_value().context("unescape column attribute")?;
        match attr.key.as_ref() {
            b"id" => {
                column.id = value
                    .parse()
                    .map_err(|err| anyhow!("parse column id: {err}"))?
            }
            b"name" => column.name = value.to_string(),
            b"width" => column.width = value.parse::<f32>().ok(),
            _ => {}
        }
    }
    Ok(column)
}

fn parse_row_id(e: &BytesStart) -> knotq_model::RowId {
    for attr in e.attributes().flatten() {
        if attr.key.as_ref() == b"id" {
            if let Ok(value) = attr.unescape_value() {
                if let Ok(id) = value.parse() {
                    return id;
                }
            }
        }
    }
    knotq_model::RowId::new()
}

fn parse_marker(value: &str) -> Result<ItemMarker> {
    ItemMarker::parse(value).map_err(|err| anyhow!(err))
}

/// Read character data up to `</end_name>`, concatenating text segments.
///
/// quick-xml emits entity references (`&lt;`, `&amp;`, …) as separate
/// [`Event::GeneralRef`] events rather than folding them into the surrounding
/// text, so they must be resolved and reassembled here.
fn read_element_text(reader: &mut Reader<&[u8]>, end_name: &[u8]) -> Result<String> {
    let mut out = String::new();
    loop {
        match reader.read_event()? {
            Event::Text(e) => out.push_str(&e.xml_content().context("decode text")?),
            Event::GeneralRef(e) => {
                if let Some(ch) = e.resolve_char_ref().context("resolve char ref")? {
                    out.push(ch);
                } else {
                    let name = e.decode().context("decode entity name")?;
                    out.push_str(match name.as_ref() {
                        "lt" => "<",
                        "gt" => ">",
                        "amp" => "&",
                        "quot" => "\"",
                        "apos" => "'",
                        other => bail!("unknown entity &{other};"),
                    });
                }
            }
            Event::CData(e) => out.push_str(&String::from_utf8_lossy(&e.into_inner())),
            Event::End(e) if e.name().as_ref() == end_name => break,
            Event::Eof => bail!("unexpected EOF in text element"),
            _ => {}
        }
    }
    Ok(out)
}

fn skip_to_end(reader: &mut Reader<&[u8]>, end_name: &[u8]) -> Result<()> {
    loop {
        match reader.read_event()? {
            Event::End(e) if e.name().as_ref() == end_name => break,
            Event::Eof => bail!("unexpected EOF"),
            _ => {}
        }
    }
    Ok(())
}

fn encode_datetime(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn parse_datetime(value: &str) -> Result<DateTime<Utc>> {
    Ok(DateTime::parse_from_rfc3339(value)?.with_timezone(&Utc))
}

fn single_rrule(repeats: &Recurrence) -> Option<&str> {
    if repeats.rrules.len() == 1
        && repeats.rdates.is_empty()
        && repeats.exdates.is_empty()
        && repeats.overrides.is_empty()
        && repeats.raw_import.is_none()
    {
        Some(&repeats.rrules[0])
    } else {
        None
    }
}

fn state_is_default(state: &[OccurrenceState]) -> bool {
    state.len() == 1 && state[0].occurrence == OccurrenceId::Single && state[0].state.is_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use knotq_model::OccurrenceId;

    #[test]
    fn roundtrips_lines_dates_and_text() {
        let mut scheme = Scheme::new("Roundtrip", 2);
        scheme
            .items
            .push(Item::new("My <special> & \"quoted\" heading"));

        let mut task = Item::new("Meet Professor see ");
        task.marker = ItemMarker::Checkbox;
        task.start = Some(chrono::Utc.with_ymd_and_hms(2026, 5, 20, 15, 0, 0).unwrap());
        task.end = Some(chrono::Utc.with_ymd_and_hms(2026, 5, 20, 16, 0, 0).unwrap());
        task.priority = Some(3);
        scheme.items.push(task);

        let xml = encode_scheme_xml(&scheme).unwrap();
        let decoded = decode_scheme_xml(&xml, Path::new("R.knotq"), scheme.id).unwrap();
        assert_eq!(decoded.items.len(), 2);
        assert_eq!(decoded.items[0].text(), "My <special> & \"quoted\" heading");
        assert_eq!(decoded.items[1].text(), "Meet Professor see ");
        assert_eq!(decoded.items[1].start, scheme.items[1].start);
        assert_eq!(decoded.items[1].priority, Some(3));
        assert_eq!(decoded.items[1].content, scheme.items[1].content);
    }

    #[test]
    fn roundtrips_table_with_checkbox_cell() {
        let mut scheme = Scheme::new("Tables", 0);
        let mut table = Table::new(2, 2);
        table.columns[0].name = "Task".to_string();
        table.columns[1].name = "Done".to_string();
        table.rows[0].cells[0].items[0].set_text("Write report");
        table.rows[0].cells[1].items[0].marker = ItemMarker::Checkbox;
        table.rows[0].cells[1].items[0].state[0].state.progress = -1;
        table.rows[1].cells[0].items = vec![
            Item::new("Ship it | now"),
            Item::new("then celebrate").with_marker(ItemMarker::Bullet),
        ];
        let mut table_item = Item::new("");
        table_item.set_table(table);
        let cell_id = table_item.table().unwrap().rows[0].cells[1].items[0].id;
        scheme.items.push(table_item);

        let xml = encode_scheme_xml(&scheme).unwrap();
        let decoded = decode_scheme_xml(&xml, Path::new("T.knotq"), scheme.id).unwrap();
        assert_eq!(decoded.items.len(), 1);
        let table = decoded.items[0].table().unwrap();
        assert_eq!(table.column_count(), 2);
        assert_eq!(table.row_count(), 2);
        assert_eq!(table.columns[0].name, "Task");
        assert_eq!(table.cell(0, 0).unwrap().first().text(), "Write report");
        let multi = table.cell(1, 0).unwrap();
        assert_eq!(multi.items.len(), 2);
        assert_eq!(multi.items[0].text(), "Ship it | now");
        assert_eq!(multi.items[1].marker, ItemMarker::Bullet);
        let done_cell = table.cell(0, 1).unwrap().first();
        assert_eq!(done_cell.marker, ItemMarker::Checkbox);
        assert_eq!(done_cell.id, cell_id);
        assert!(done_cell.single_state().is_done());
        assert_eq!(done_cell.state[0].occurrence, OccurrenceId::Single);
    }

    #[test]
    fn empty_scheme_and_empty_item_roundtrip() {
        let mut scheme = Scheme::new("Empty", 0);
        scheme.items.push(Item::new(""));
        let xml = encode_scheme_xml(&scheme).unwrap();
        let decoded = decode_scheme_xml(&xml, Path::new("E.knotq"), scheme.id).unwrap();
        assert_eq!(decoded.items.len(), 1);
        assert!(decoded.items[0].is_content_empty());
    }

    #[test]
    fn dotted_marker_subtypes_decode_as_base_markers() {
        assert_eq!(parse_marker("bullet.disc").unwrap(), ItemMarker::Bullet);
        assert_eq!(
            parse_marker("numbered.alphabet").unwrap(),
            ItemMarker::Numbered
        );
        assert!(parse_marker("list.alphabet").is_err());
    }

    #[test]
    fn strips_illegal_xml_control_chars_and_preserves_metacharacters() {
        let mut scheme = Scheme::new("Sanitize", 0);
        // Text content: XML metacharacters must survive escaping; control
        // characters illegal in XML 1.0 (NUL, 0x01, 0x08, 0x0B, 0x0C, 0x1F) must
        // be stripped, not written raw — otherwise the file fails to parse on the
        // next load and the scheme is silently lost.
        scheme
            .items
            .push(Item::new("a<b>&\"c\u{0}\u{1}\u{8}\u{B}\u{C}\u{1F}d"));

        // A table column name exercises the attribute escape + sanitize path.
        let mut table = Table::new(1, 1);
        table.columns[0].name = "N<a>m&\"e\u{0}\u{7}".to_string();
        let mut block = Item::new("");
        block.set_table(table);
        scheme.items.push(block);

        let xml = encode_scheme_xml(&scheme).unwrap();
        // The writer must never emit a character illegal in XML 1.0.
        assert!(
            xml.chars().all(is_xml_char),
            "encoded XML contains an illegal control character"
        );
        // And it must re-parse cleanly, proving the output is well-formed.
        let decoded = decode_scheme_xml(&xml, Path::new("S.knotq"), scheme.id).unwrap();
        assert_eq!(decoded.items[0].text(), "a<b>&\"cd");
        assert_eq!(
            decoded.items[1].table().unwrap().columns[0].name,
            "N<a>m&\"e"
        );
    }
}
