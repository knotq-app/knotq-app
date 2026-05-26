pub(super) use knotq_ui::text_util::{
    next_char_boundary, next_word_offset, previous_char_boundary, previous_word_offset,
    word_range_at,
};

#[cfg(test)]
mod tests {
    use knotq_ui::text_util::{next_word_offset, previous_word_offset, word_range_at};

    #[test]
    fn word_navigation_matches_editor_boundaries() {
        let text = "alpha beta.gamma";
        assert_eq!(previous_word_offset(text, "alpha beta".len()), 6);
        assert_eq!(next_word_offset(text, 0), 5);
        assert_eq!(next_word_offset(text, 6), 10);
        assert_eq!(word_range_at(text, 7), 6..10);
        assert_eq!(word_range_at(text, 10), 10..11);
    }

    #[test]
    fn word_range_at_line_end_selects_previous_word_not_newline() {
        let text = "# ICPC\nJhala Office Hours";
        assert_eq!(word_range_at(text, "# ICPC".len()), 2.."# ICPC".len());
    }
}
