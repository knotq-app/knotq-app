use anyhow::{bail, Context, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use knotq_model::{
    ExternalItemSource, Item, ItemId, ItemMarker, OccurrenceId, OccurrenceState, Recurrence,
    Scheme, SchemeId,
};
use std::collections::BTreeMap;
use std::path::Path;

use crate::scheme_file::SchemeFile;

const ATTR_PREFIX: &str = "!knotq{";

pub(crate) fn encode_scheme_file(scheme: &Scheme) -> Result<String> {
    let mut out = String::new();
    for item in &scheme.items {
        out.push_str(&encode_item(item)?);
        out.push('\n');
    }
    Ok(out)
}

pub(crate) fn decode_scheme_file(raw: &str, path: &Path, id: SchemeId) -> Result<SchemeFile> {
    let mut items = Vec::new();
    for (line_index, line) in raw.lines().enumerate() {
        items.push(decode_item(line).with_context(|| {
            format!("parse item line {} in {}", line_index + 1, path.display())
        })?);
    }

    Ok(SchemeFile { id, items })
}

fn encode_item(item: &Item) -> Result<String> {
    let indent = "  ".repeat(item.indent as usize);
    let checked = item.marker == ItemMarker::Checkbox && item.single_state().is_done();
    let marker = match item.marker {
        ItemMarker::Blank => "",
        ItemMarker::Bullet => "- ",
        ItemMarker::Numbered => "1. ",
        ItemMarker::Checkbox if checked => "- [x] ",
        ItemMarker::Checkbox => "- [ ] ",
    };
    let mut body = escape_text(&item.text);
    let attrs = encode_item_attrs(item, checked)?;
    if !attrs.is_empty() {
        if !body.is_empty() {
            body.push(' ');
        }
        body.push_str(&format_attr_block(&attrs)?);
    }
    Ok(format!("{indent}{marker}{body}"))
}

fn encode_item_attrs(
    item: &Item,
    checked_marker_represents_state: bool,
) -> Result<Vec<(&'static str, String)>> {
    let mut attrs = Vec::new();
    if item_needs_stable_id(item, checked_marker_represents_state) {
        attrs.push(("id", item.id.to_string()));
    }
    if let Some(start) = item.start {
        attrs.push(("start", encode_datetime(start)));
    }
    if let Some(end) = item.end {
        attrs.push(("end", encode_datetime(end)));
    }
    if let Some(available) = item.available {
        attrs.push(("available", encode_datetime(available)));
    }
    if let Some(priority) = item.priority {
        attrs.push(("priority", priority.to_string()));
    }
    if let Some(repeats) = &item.repeats {
        if let Some(rrule) = single_rrule(repeats) {
            attrs.push(("rrule", rrule.to_string()));
        } else {
            attrs.push(("repeats", serde_json::to_string(repeats)?));
        }
    }
    if !state_is_default(&item.state)
        && !(checked_marker_represents_state && state_is_single_done(&item.state))
    {
        attrs.push(("state", serde_json::to_string(&item.state)?));
    }
    if !item.media.is_empty() {
        attrs.push(("media", serde_json::to_string(&item.media)?));
    }
    if let Some(external) = &item.external {
        attrs.push(("external", serde_json::to_string(external)?));
    }
    Ok(attrs)
}

fn decode_item(line: &str) -> Result<Item> {
    let (indent, rest) = split_indent(line);
    let (marker, checked, body) = split_marker(rest);
    let (text, attrs) = split_trailing_attrs(body)?;
    let mut item = Item::new(unescape_text(text)?);
    item.indent = indent;
    item.marker = marker;
    if let Some(id) = attrs.get("id") {
        item.id = id.parse::<ItemId>().context("parse item id")?;
    }
    if checked && marker == ItemMarker::Checkbox {
        item.state[0].state.progress = -1;
    }
    if let Some(start) = attrs.get("start") {
        item.start = Some(parse_datetime(start).context("parse start")?);
    }
    if let Some(end) = attrs.get("end") {
        item.end = Some(parse_datetime(end).context("parse end")?);
    }
    if let Some(available) = attrs.get("available") {
        item.available = Some(parse_datetime(available).context("parse available")?);
    }
    if let Some(priority) = attrs.get("priority") {
        item.priority = Some(priority.parse::<u8>().context("parse priority")?);
    }
    if let Some(rrule) = attrs.get("rrule") {
        item.repeats = Some(Recurrence {
            rrules: vec![rrule.clone()],
            ..Default::default()
        });
    }
    if let Some(repeats) = attrs.get("repeats") {
        item.repeats = Some(serde_json::from_str(repeats).context("parse repeats")?);
    }
    if let Some(state) = attrs.get("state") {
        item.state = serde_json::from_str(state).context("parse state")?;
    }
    if let Some(media) = attrs.get("media") {
        item.media = serde_json::from_str(media).context("parse media")?;
    }
    if let Some(external) = attrs.get("external") {
        item.external =
            Some(serde_json::from_str::<ExternalItemSource>(external).context("parse external")?);
    }
    item.enforce_marker_constraints();
    Ok(item)
}

fn split_indent(line: &str) -> (u8, &str) {
    let mut spaces = 0;
    for ch in line.chars() {
        if ch == ' ' {
            spaces += 1;
        } else {
            break;
        }
    }
    let indent_spaces = spaces / 2 * 2;
    (
        (indent_spaces / 2).min(u8::MAX as usize) as u8,
        &line[indent_spaces..],
    )
}

fn split_marker(rest: &str) -> (ItemMarker, bool, &str) {
    if let Some(body) = rest.strip_prefix("- [ ] ") {
        return (ItemMarker::Checkbox, false, body);
    }
    if rest == "- [ ]" {
        return (ItemMarker::Checkbox, false, "");
    }
    if let Some(body) = rest
        .strip_prefix("- [x] ")
        .or_else(|| rest.strip_prefix("- [X] "))
    {
        return (ItemMarker::Checkbox, true, body);
    }
    if rest == "- [x]" || rest == "- [X]" {
        return (ItemMarker::Checkbox, true, "");
    }
    if let Some(body) = rest.strip_prefix("- ").or_else(|| rest.strip_prefix("* ")) {
        return (ItemMarker::Bullet, false, body);
    }
    if let Some(body) = numbered_body(rest) {
        return (ItemMarker::Numbered, false, body);
    }
    (ItemMarker::Blank, false, rest)
}

fn numbered_body(rest: &str) -> Option<&str> {
    let dot = rest.find('.')?;
    if dot == 0 || !rest[..dot].chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    rest[dot + 1..].strip_prefix(' ')
}

fn split_trailing_attrs(body: &str) -> Result<(&str, BTreeMap<String, String>)> {
    let Some(start) = find_trailing_attr_block(body) else {
        return Ok((body, BTreeMap::new()));
    };
    let attrs = parse_attr_block(&body[start..])?;
    let text_end = body[..start]
        .char_indices()
        .last()
        .filter(|(_, ch)| ch.is_whitespace())
        .map(|(idx, _)| idx)
        .unwrap_or(start);
    Ok((&body[..text_end], attrs))
}

fn escape_text(text: &str) -> String {
    let mut out = text.replace('\\', "\\\\");
    if out.starts_with(ATTR_PREFIX) {
        out.insert(0, '\\');
    }
    if let Some(start) = find_trailing_attr_block(&out) {
        out.insert(start, '\\');
    }
    out
}

fn unescape_text(text: &str) -> Result<String> {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            let Some(next) = chars.next() else {
                out.push(ch);
                break;
            };
            if next == '\\' || next == '!' {
                out.push(next);
            } else {
                out.push(ch);
                out.push(next);
            }
        } else {
            out.push(ch);
        }
    }
    Ok(out)
}

fn find_trailing_attr_block(input: &str) -> Option<usize> {
    if !input.ends_with('}') {
        return None;
    }
    for (idx, _) in input.match_indices(ATTR_PREFIX) {
        if is_escaped(input, idx) {
            continue;
        }
        if idx > 0
            && !input[..idx]
                .chars()
                .last()
                .is_some_and(|ch| ch.is_whitespace())
        {
            continue;
        }
        if parse_attr_block(&input[idx..]).is_ok() {
            return Some(idx);
        }
    }
    None
}

fn is_escaped(input: &str, idx: usize) -> bool {
    let mut count = 0;
    for ch in input[..idx].chars().rev() {
        if ch == '\\' {
            count += 1;
        } else {
            break;
        }
    }
    count % 2 == 1
}

fn parse_attr_block(input: &str) -> Result<BTreeMap<String, String>> {
    if !input.starts_with(ATTR_PREFIX) || !input.ends_with('}') {
        bail!("expected !knotq{{...}}");
    }
    let content = &input[ATTR_PREFIX.len()..input.len() - 1];
    let mut attrs = BTreeMap::new();
    let bytes = content.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i == bytes.len() {
            break;
        }
        let key_start = i;
        while i < bytes.len()
            && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'-')
        {
            i += 1;
        }
        if key_start == i {
            bail!("expected attribute key");
        }
        let key = &content[key_start..i];
        if i >= bytes.len() || bytes[i] != b'=' {
            bail!("expected = after attribute key {key:?}");
        }
        i += 1;
        if i >= bytes.len() {
            bail!("missing attribute value for {key:?}");
        }
        let value = if bytes[i] == b'"' {
            let value_start = i;
            i += 1;
            let mut escaped = false;
            while i < bytes.len() {
                let byte = bytes[i];
                if escaped {
                    escaped = false;
                } else if byte == b'\\' {
                    escaped = true;
                } else if byte == b'"' {
                    i += 1;
                    break;
                }
                i += 1;
            }
            let token = &content[value_start..i];
            serde_json::from_str::<String>(token)
                .with_context(|| format!("parse quoted attribute value for {key:?}"))?
        } else {
            let value_start = i;
            while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            content[value_start..i].to_string()
        };
        attrs.insert(key.to_string(), value);
    }
    Ok(attrs)
}

fn format_attr_block(attrs: &[(&'static str, String)]) -> Result<String> {
    let mut out = String::from(ATTR_PREFIX);
    for (index, (key, value)) in attrs.iter().enumerate() {
        if index > 0 {
            out.push(' ');
        }
        out.push_str(key);
        out.push('=');
        out.push_str(&serde_json::to_string(value)?);
    }
    out.push('}');
    Ok(out)
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

fn state_is_single_done(state: &[OccurrenceState]) -> bool {
    state.len() == 1 && state[0].occurrence == OccurrenceId::Single && state[0].state.is_done()
}

fn item_needs_stable_id(item: &Item, checked_marker_represents_state: bool) -> bool {
    let _ = (item, checked_marker_represents_state);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use knotq_model::{CalendarProvider, ExternalItemSource, ImageAssetFormat, ItemMedia};
    use uuid::Uuid;

    #[test]
    fn escapes_attribute_like_text_at_start_and_end() {
        let mut scheme = Scheme::new("Escapes", 0);
        scheme.items.push(Item::new("!knotq{not_attrs=true}"));
        scheme.items.push(Item::new("literal !knotq{id=\"fake\"}"));
        scheme.items.push(Item::new(r"C:\Users\me"));

        let encoded = encode_scheme_file(&scheme).unwrap();
        assert!(encoded.contains("\\!knotq{not_attrs=true}"));
        assert!(encoded.contains("literal \\!knotq{id=\"fake\"}"));

        let decoded = decode_scheme_file(&encoded, Path::new("Escapes.knotq"), scheme.id).unwrap();
        assert_eq!(decoded.id, scheme.id);
        assert_eq!(decoded.items[0].text, "!knotq{not_attrs=true}");
        assert_eq!(decoded.items[1].text, "literal !knotq{id=\"fake\"}");
        assert_eq!(decoded.items[2].text, r"C:\Users\me");
    }

    #[test]
    fn roundtrips_markers_dates_recurrence_and_json_attrs() {
        let mut scheme = Scheme::new("Roundtrip", 2);
        let mut item = Item::new("Meet Professor");
        item.marker = ItemMarker::Checkbox;
        item.start = Some(Utc.with_ymd_and_hms(2026, 5, 20, 15, 0, 0).unwrap());
        item.end = Some(Utc.with_ymd_and_hms(2026, 5, 20, 16, 0, 0).unwrap());
        item.repeats = Some(Recurrence {
            rrules: vec!["FREQ=WEEKLY;BYDAY=WE".to_string()],
            ..Default::default()
        });
        item.media.push(ItemMedia::Image {
            asset: Uuid::new_v4(),
            format: ImageAssetFormat::Png,
            width: Some(320),
            height: Some(180),
        });
        item.external = Some(ExternalItemSource {
            provider: CalendarProvider::Google,
            account_id: "google".to_string(),
            calendar_id: "work".to_string(),
            event_id: "event-1".to_string(),
            instance_id: None,
            updated_at: None,
        });
        scheme.items.push(item);

        let encoded = encode_scheme_file(&scheme).unwrap();
        assert!(encoded.contains("- [ ] Meet Professor"));
        assert!(encoded.contains("rrule=\"FREQ=WEEKLY;BYDAY=WE\""));
        assert!(encoded.contains("media="));
        assert!(encoded.contains("external="));

        assert!(encoded.contains(&format!("id=\"{}\"", scheme.items[0].id)));

        let decoded =
            decode_scheme_file(&encoded, Path::new("Roundtrip.knotq"), scheme.id).unwrap();
        assert_eq!(decoded.id, scheme.id);
        assert_eq!(decoded.items.len(), 1);
        let decoded = &decoded.items[0];
        let original = &scheme.items[0];
        assert_eq!(decoded.id, original.id);
        assert_eq!(decoded.text, original.text);
        assert_eq!(decoded.marker, original.marker);
        assert_eq!(decoded.start, original.start);
        assert_eq!(decoded.end, original.end);
        assert_eq!(decoded.repeats, original.repeats);
        assert_eq!(decoded.media, original.media);
        assert_eq!(decoded.external, original.external);
    }
}
