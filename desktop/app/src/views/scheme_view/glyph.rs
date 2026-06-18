use super::*;

#[derive(Clone, Copy)]
pub(super) enum ToolbarGlyph {
    Plain,
    Checkbox,
    Bullet,
    Numbered,
    Heading,
    Table,
    Unindent,
    Indent,
}

pub(super) fn toolbar_glyph(glyph: ToolbarGlyph, active: bool, c: Theme) -> gpui::AnyElement {
    let color = if active {
        token_hsla(c.toolbar_chip_selected_text)
    } else {
        token_hsla(c.toolbar_chip_muted)
    };
    match glyph {
        ToolbarGlyph::Plain => div()
            .w(px(12.0))
            .h(px(if active { 3.0 } else { 1.0 }))
            .rounded(px(1.0))
            .bg(color)
            .into_any_element(),
        ToolbarGlyph::Checkbox => {
            let base = div()
                .w(px(12.0))
                .h(px(12.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(3.0))
                .border_color(color);
            if active {
                base.border_2()
                    .child(
                        Icon::new(IconName::Check)
                            .with_size(px(9.0))
                            .text_color(color),
                    )
                    .into_any_element()
            } else {
                base.border_1().into_any_element()
            }
        }
        ToolbarGlyph::Bullet => div()
            .w(px(if active { 7.0 } else { 5.0 }))
            .h(px(if active { 7.0 } else { 5.0 }))
            .rounded(px(4.0))
            .bg(color)
            .into_any_element(),
        ToolbarGlyph::Numbered => div()
            .font_family(FONT_UI)
            .font_weight(if active {
                gpui::FontWeight::BOLD
            } else {
                gpui::FontWeight::MEDIUM
            })
            .text_size(px(12.0))
            .line_height(px(12.0))
            .text_color(color)
            .child("1.")
            .into_any_element(),
        ToolbarGlyph::Heading => div()
            .font_family(FONT_UI)
            .font_weight(if active {
                gpui::FontWeight::BOLD
            } else {
                gpui::FontWeight::MEDIUM
            })
            .text_size(px(13.0))
            .line_height(px(13.0))
            .text_color(color)
            .child("#")
            .into_any_element(),
        // A 14×12 table: a header band (top divider) over two body columns
        // (one centered vertical divider). Reads clearly as a table and is
        // symmetric within the 24px chip.
        ToolbarGlyph::Table => div()
            .relative()
            .w(px(14.0))
            .h(px(12.0))
            .rounded(px(2.0))
            .border_1()
            .border_color(color)
            // Header divider, full inner width.
            .child(
                div()
                    .absolute()
                    .left(px(0.0))
                    .right(px(0.0))
                    .top(px(3.0))
                    .h(px(1.0))
                    .bg(color),
            )
            // Body column divider, centered, below the header.
            .child(
                div()
                    .absolute()
                    .top(px(4.0))
                    .bottom(px(0.0))
                    .left(px(6.0))
                    .w(px(1.0))
                    .bg(color),
            )
            .into_any_element(),
        ToolbarGlyph::Unindent => div()
            .font_family(FONT_MONO)
            .font_weight(if active {
                gpui::FontWeight::BOLD
            } else {
                gpui::FontWeight::MEDIUM
            })
            .text_size(px(12.0))
            .line_height(px(12.0))
            .text_color(color)
            .child("<")
            .into_any_element(),
        ToolbarGlyph::Indent => div()
            .font_family(FONT_MONO)
            .font_weight(if active {
                gpui::FontWeight::BOLD
            } else {
                gpui::FontWeight::MEDIUM
            })
            .text_size(px(12.0))
            .line_height(px(12.0))
            .text_color(color)
            .child(">")
            .into_any_element(),
    }
}
