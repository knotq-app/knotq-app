use super::*;

pub(super) fn toolbar_separator(color: u32) -> gpui::AnyElement {
    div()
        .w(px(1.0))
        .h(px(16.0))
        .mx(px(3.0))
        .bg(token_rgba(color))
        .into_any_element()
}

pub(super) fn toolbar_glyph_button(
    id: &'static str,
    active: bool,
    glyph: ToolbarGlyph,
    c: Theme,
    tooltip: &'static str,
    editor: Entity<SchemeEditor>,
    listener: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::AnyElement {
    div()
        .id(id)
        .w(px(24.0))
        .h(px(23.0))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, toolbar_refocus_listener(editor))
        .on_click(listener)
        .tooltip(move |window, cx| Tooltip::new(tooltip).build(window, cx))
        .child(toolbar_glyph(glyph, active, c))
        .into_any_element()
}

pub(super) fn toolbar_bold_button(
    active: bool,
    c: Theme,
    tooltip: &'static str,
    editor: Entity<SchemeEditor>,
    listener: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::AnyElement {
    div()
        .id("scheme-toolbar-bold")
        .w(px(24.0))
        .h(px(23.0))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, toolbar_refocus_listener(editor))
        .on_click(listener)
        .tooltip(move |window, cx| Tooltip::new(tooltip).build(window, cx))
        .child(
            div()
                .font_family(FONT_UI)
                .font_weight(if active {
                    gpui::FontWeight::BOLD
                } else {
                    gpui::FontWeight::MEDIUM
                })
                .text_size(px(13.0))
                .line_height(px(13.0))
                .text_color(if active {
                    token_hsla(c.toolbar_chip_selected_text)
                } else {
                    token_hsla(c.toolbar_chip_muted)
                })
                .child("B"),
        )
        .into_any_element()
}

pub(super) fn toolbar_italic_button(
    active: bool,
    c: Theme,
    tooltip: &'static str,
    editor: Entity<SchemeEditor>,
    listener: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::AnyElement {
    div()
        .id("scheme-toolbar-italic")
        .w(px(24.0))
        .h(px(23.0))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, toolbar_refocus_listener(editor))
        .on_click(listener)
        .tooltip(move |window, cx| Tooltip::new(tooltip).build(window, cx))
        .child(
            div()
                .font_family(FONT_UI)
                .font_weight(if active {
                    gpui::FontWeight::BOLD
                } else {
                    gpui::FontWeight::MEDIUM
                })
                .italic()
                .text_size(px(13.0))
                .line_height(px(13.0))
                .text_color(if active {
                    token_hsla(c.toolbar_chip_selected_text)
                } else {
                    token_hsla(c.toolbar_chip_muted)
                })
                .child("I"),
        )
        .into_any_element()
}

pub(super) fn toolbar_strikethrough_button(
    active: bool,
    c: Theme,
    tooltip: &'static str,
    editor: Entity<SchemeEditor>,
    listener: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::AnyElement {
    div()
        .id("scheme-toolbar-strikethrough")
        .w(px(24.0))
        .h(px(23.0))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, toolbar_refocus_listener(editor))
        .on_click(listener)
        .tooltip(move |window, cx| Tooltip::new(tooltip).build(window, cx))
        .child(
            div()
                .font_family(FONT_UI)
                .font_weight(if active {
                    gpui::FontWeight::BOLD
                } else {
                    gpui::FontWeight::MEDIUM
                })
                .line_through()
                .text_size(px(13.0))
                .line_height(px(13.0))
                .text_color(if active {
                    token_hsla(c.toolbar_chip_selected_text)
                } else {
                    token_hsla(c.toolbar_chip_muted)
                })
                .child("S"),
        )
        .into_any_element()
}

pub(super) fn toolbar_highlight_button(
    active: bool,
    c: Theme,
    tooltip: &'static str,
    editor: Entity<SchemeEditor>,
    listener: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::AnyElement {
    div()
        .id("scheme-toolbar-highlight")
        .w(px(24.0))
        .h(px(23.0))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, toolbar_refocus_listener(editor))
        .on_click(listener)
        .tooltip(move |window, cx| Tooltip::new(tooltip).build(window, cx))
        .child(if active {
            div()
                .px(px(3.0))
                .rounded(px(3.0))
                .bg(token_rgba(c.highlight_bg))
                .child(
                    div()
                        .font_family(FONT_UI)
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_size(px(13.0))
                        .line_height(px(13.0))
                        .text_color(token_hsla(c.toolbar_chip_selected_text))
                        .child("H"),
                )
                .into_any_element()
        } else {
            div()
                .font_family(FONT_UI)
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_size(px(13.0))
                .line_height(px(13.0))
                .text_color(token_hsla(c.toolbar_chip_muted))
                .child("H")
                .into_any_element()
        })
        .into_any_element()
}

pub(super) fn toolbar_date_button(
    id: &'static str,
    label: &'static str,
    active: bool,
    c: Theme,
    tooltip: &'static str,
    editor: Entity<SchemeEditor>,
    listener: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::AnyElement {
    div()
        .id(id)
        .h(px(23.0))
        .px(px(7.0))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, toolbar_refocus_listener(editor))
        .on_click(listener)
        .tooltip(move |window, cx| Tooltip::new(tooltip).build(window, cx))
        .child(
            div()
                .font_family(FONT_MONO)
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_size(px(10.0))
                .line_height(px(11.0))
                .text_color(if active {
                    token_hsla(c.toolbar_chip_selected_text)
                } else {
                    token_hsla(c.toolbar_chip_muted)
                })
                .child(label),
        )
        .into_any_element()
}

pub(super) fn toolbar_refocus_listener(
    editor: Entity<SchemeEditor>,
) -> impl Fn(&gpui::MouseDownEvent, &mut Window, &mut App) + 'static {
    move |_, window, cx| {
        window.prevent_default();
        editor.update(cx, |editor, cx| editor.focus(window, cx));
        cx.stop_propagation();
    }
}
