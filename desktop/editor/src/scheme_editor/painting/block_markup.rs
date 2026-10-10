use super::super::*;

/// How far left of the text a quote's bar sits, and how wide it is.
const QUOTE_BAR_GAP: f32 = 9.0;
const QUOTE_BAR_WIDTH: f32 = 2.5;
/// Room kept clear to the right of a rule, matching the text's own margin.
const RULE_RIGHT_INSET: f32 = 18.0;

impl SchemeEditor {
    /// Paints what a whole-line markdown form draws beside or instead of its
    /// text: the bar of a `> quote`, and the line of a `---` rule. The rule is
    /// only drawn while its dashes are collapsed — on the cursor's line they
    /// are shown as typed.
    pub(in crate::scheme_editor) fn paint_block_markup(
        &self,
        row: usize,
        line_origin: Point<Pixels>,
        bounds: Bounds<Pixels>,
        window: &mut Window,
    ) {
        let Some(line) = self.line_range(row).and_then(|range| self.text.get(range)) else {
            return;
        };
        if is_markdown_rule(line) {
            if self.row_reveals_markers(row) {
                return;
            }
            let y = line_origin.y + self.line_map.row_line_height(row) / 2.0;
            let right = bounds.right() - px(RULE_RIGHT_INSET);
            if right > line_origin.x {
                window.paint_quad(fill(
                    Bounds::new(
                        point(line_origin.x, y),
                        size(right - line_origin.x, px(1.0)),
                    ),
                    token_rgba(self.theme.border_strong),
                ));
            }
        } else if markdown_quote_marker_range(line).is_some() {
            window.paint_quad(fill(
                Bounds::new(
                    point(line_origin.x - px(QUOTE_BAR_GAP), line_origin.y + px(2.0)),
                    size(
                        px(QUOTE_BAR_WIDTH),
                        (self.line_map.line_text_height(row) - px(4.0)).max(px(4.0)),
                    ),
                ),
                token_rgba(self.theme.border_strong),
            ));
        }
    }
}
