use knotq_editor_core::{TextLocation, TextSelection};

#[test]
fn selection_orders_anchor_and_head() {
    let selection = TextSelection {
        anchor: TextLocation { row: 2, col: 0 },
        head: TextLocation { row: 1, col: 3 },
    };

    assert!(selection.reversed());
    assert_eq!(
        selection.ordered(),
        (
            TextLocation { row: 1, col: 3 },
            TextLocation { row: 2, col: 0 }
        )
    );
}
