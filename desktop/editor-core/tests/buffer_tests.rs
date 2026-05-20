use knotq_editor_core::{clean_line_text, line_ranges, EditorBuffer};
use knotq_model::Item;

#[test]
fn buffer_builds_text_and_rows_from_items() {
    let first = Item::new(" first");
    let first_id = first.id;
    let second = Item::new("second");

    let buffer = EditorBuffer::from_items(&[first, second]);

    assert_eq!(buffer.text, "first\nsecond");
    assert_eq!(buffer.rows[0].item_id, first_id);
    assert_eq!(buffer.rows[0].text_range, 0..5);
    assert_eq!(clean_line_text("\t child"), "child");
}

#[test]
fn line_ranges_include_empty_trailing_line() {
    assert_eq!(line_ranges("a\n"), vec![0..1, 2..2]);
}
