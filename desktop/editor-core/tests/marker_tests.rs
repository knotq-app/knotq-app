use knotq_editor_core::{marker_prefix, parse_marker_content, parse_marker_prefix};
use knotq_model::ItemMarker;

#[test]
fn marker_prefix_round_trips_common_markers() {
    assert_eq!(marker_prefix(ItemMarker::Checkbox, 1), "- [ ] ");
    assert_eq!(
        parse_marker_content("- [ ] task"),
        (ItemMarker::Checkbox, "task")
    );
    assert_eq!(parse_marker_content("- task"), (ItemMarker::Bullet, "task"));
    assert_eq!(
        parse_marker_content("3. task"),
        (ItemMarker::Numbered, "task")
    );
    assert_eq!(parse_marker_prefix("plain"), (ItemMarker::Blank, 0));
}
