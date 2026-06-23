//! Hand-written XML encoder for scheme files.
//!
//! The encoder writes through quick-xml's `Writer`, so all element, attribute,
//! and text escaping is the library's responsibility.

use anyhow::Result;
use knotq_model::{ImageAssetFormat, ImageInline, Item, ItemContent, OccurrenceId, OccurrenceState, Recurrence, Scheme, Table};
use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event};
use quick_xml::writer::Writer;
use std::io::Write;

use super::shared::{encode_datetime, strip_invalid_xml_chars};

const SCHEME_VERSION: &str = "1";

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
