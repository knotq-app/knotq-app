use knotq_editor_core::{soft_wrap, WrapLine};

#[test]
fn soft_wrap_breaks_on_words_when_possible() {
    let wrapped = soft_wrap("alpha beta gamma", 10.0, |text| text.len() as f32);

    assert_eq!(
        wrapped,
        vec![
            WrapLine {
                text_range: 0..6,
                is_continuation: false,
            },
            WrapLine {
                text_range: 6..16,
                is_continuation: true,
            },
        ]
    );
}
