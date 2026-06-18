use super::*;

#[derive(Clone, Copy)]
pub(super) enum ToolbarGlyph {
    Plain,
    Checkbox,
    Bullet,
    Numbered,
    Heading,
    Image,
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
        ToolbarGlyph::Image => div()
            .relative()
            .w(px(15.0))
            .h(px(12.0))
            .rounded(px(2.0))
            .border_1()
            .border_color(color)
            .child(
                div()
                    .absolute()
                    .top(px(2.0))
                    .right(px(2.0))
                    .w(px(3.0))
                    .h(px(3.0))
                    .rounded(px(2.0))
                    .bg(color),
            )
            .child(
                div()
                    .absolute()
                    .left(px(2.0))
                    .right(px(2.0))
                    .bottom(px(2.0))
                    .h(px(1.0))
                    .bg(color),
            )
            .child(
                div()
                    .absolute()
                    .left(px(4.0))
                    .bottom(px(2.0))
                    .w(px(1.0))
                    .h(px(3.0))
                    .bg(color),
            )
            .child(
                div()
                    .absolute()
                    .left(px(7.0))
                    .bottom(px(2.0))
                    .w(px(1.0))
                    .h(px(5.0))
                    .bg(color),
            )
            .into_any_element(),
        ToolbarGlyph::Table => div()
            .relative()
            .w(px(14.0))
            .h(px(13.0))
            .rounded(px(2.0))
            .border_1()
            .border_color(color)
            .child(
                div()
                    .absolute()
                    .top(px(1.0))
                    .bottom(px(1.0))
                    .left(px(4.0))
                    .w(px(1.0))
                    .bg(color),
            )
            .child(
                div()
                    .absolute()
                    .top(px(1.0))
                    .bottom(px(1.0))
                    .left(px(8.0))
                    .w(px(1.0))
                    .bg(color),
            )
            .child(
                div()
                    .absolute()
                    .left(px(1.0))
                    .right(px(1.0))
                    .top(px(4.0))
                    .h(px(1.0))
                    .bg(color),
            )
            .child(
                div()
                    .absolute()
                    .left(px(1.0))
                    .right(px(1.0))
                    .top(px(8.0))
                    .h(px(1.0))
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
