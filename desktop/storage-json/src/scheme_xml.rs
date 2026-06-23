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

mod read;
mod shared;
mod write;

pub(crate) use read::decode_scheme_xml;
pub(crate) use write::encode_scheme_xml;

#[cfg(test)]
use read::parse_marker;
#[cfg(test)]
use shared::is_xml_char;

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use knotq_model::{Item, ItemMarker, OccurrenceId, Scheme, Table};
    use std::path::Path;

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
