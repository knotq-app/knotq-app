//! quick-xml pull-parser decoder for scheme files.

use anyhow::{anyhow, bail, Context, Result};
use knotq_model::{
    ExternalItemSource, ImageAssetFormat, ImageInline, Inline, Item, ItemContent, ItemId,
    ItemMarker, Recurrence, SchemeId, Table, TableCell, TableColumn, TableRow,
};
use quick_xml::events::BytesStart;
use quick_xml::reader::Reader;
use quick_xml::events::Event;
use std::path::Path;

use crate::scheme_file::SchemeFile;

use super::shared::parse_datetime;

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

pub(super) fn parse_marker(value: &str) -> Result<ItemMarker> {
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
