use super::*;

pub(super) fn special_row(
    label: &'static str,
    square_color: u32,
    selected: bool,
    t: Theme,
    context_menu_open: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    div()
        .id(SharedString::new_static(label))
        .flex()
        .items_center()
        .gap(px(NAV_ICON_GAP))
        .w_full()
        .h(px(NAV_ROW_HEIGHT))
        .pl(px(NAV_ROW_INDENT_BASE))
        .pr(px(4.0))
        .rounded(px(3.0))
        .cursor_pointer()
        .when(selected, move |s| s.bg(token_rgba(t.row_selected)))
        .when(!selected && !context_menu_open, move |s| {
            s.hover(move |h| h.bg(token_rgba(t.row_hover)))
        })
        .on_click(on_click)
        .child(nav_icon_slot(
            div()
                .relative()
                .left(px(1.0))
                .w(px(SCHEME_SQUARE_SIZE))
                .h(px(SCHEME_SQUARE_SIZE))
                .rounded(px(2.0))
                .flex_shrink_0()
                .bg(token_rgba(square_color))
                .into_any_element(),
        ))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .whitespace_nowrap()
                .text_size(px(SIDEBAR_TEXT_SIZE))
                .line_height(px(SIDEBAR_LINE_HEIGHT))
                .font_family(FONT_UI)
                .text_color(token_hsla(t.text_primary))
                .child(label),
        )
}

pub(super) fn zed_folder_icon(expanded: bool, t: Theme) -> Icon {
    Icon::empty()
        .path(if expanded {
            ZED_FOLDER_ICON
        } else {
            ZED_FOLDER_OPEN_ICON
        })
        .with_size(px(FOLDER_ICON_SIZE))
        .text_color(token_hsla(t.text_dim))
}

pub(super) fn empty_folder_placeholder(
    folder_id: FolderId,
    depth: usize,
    t: Theme,
) -> gpui::AnyElement {
    let indent = NAV_ROW_INDENT_BASE + depth as f32 * 9.0 + NAV_ICON_SLOT + NAV_ICON_GAP;
    div()
        .id(SharedString::from(format!(
            "empty-folder-row-{}",
            folder_id
        )))
        .flex()
        .items_center()
        .w_full()
        .min_w_0()
        .h(px(NAV_ROW_HEIGHT))
        .pl(px(indent))
        .pr(px(4.0))
        .rounded(px(5.0))
        .text_size(px(12.0))
        .text_color(token_hsla(t.text_muted))
        .child(div().flex_1().min_w_0().child("No items"))
        .into_any_element()
}

pub(super) fn nav_icon_slot(icon: gpui::AnyElement) -> gpui::AnyElement {
    div()
        .w(px(NAV_ICON_SLOT))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .child(icon)
        .into_any_element()
}

pub(super) fn inline_rename_input(
    input: Entity<SingleLineEditor>,
    error: Option<String>,
    t: Theme,
) -> gpui::AnyElement {
    let has_error = error.is_some();
    let warning_border = if t.is_dark { 0xe2c16cff } else { 0x9a6a00ff };
    let warning_bg = if t.is_dark { 0xe2c16c18 } else { 0xffdf8a36 };
    div()
        .flex_1()
        .min_w_0()
        .when(!has_error, |s| s.h_full().overflow_hidden())
        .when(has_error, |s| s.flex().flex_col().gap(px(3.0)))
        .text_size(px(SIDEBAR_TEXT_SIZE))
        .font_family(FONT_UI)
        .font_weight(gpui::FontWeight::NORMAL)
        .text_color(token_hsla(t.text_primary))
        .line_height(px(SIDEBAR_LINE_HEIGHT))
        .child(
            div()
                .w_full()
                .h(px(NAV_ROW_HEIGHT))
                .flex()
                .items_center()
                .child(input),
        )
        .when_some(error, move |s, message| {
            s.child(
                div()
                    .w_full()
                    .px(px(8.0))
                    .py(px(6.0))
                    .rounded(px(3.0))
                    .border_1()
                    .border_color(token_rgba(warning_border))
                    .bg(token_rgba(warning_bg))
                    .text_size(px(12.0))
                    .line_height(px(17.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(token_hsla(warning_border))
                    .child(message),
            )
        })
        .into_any_element()
}

pub(super) fn footer_button(
    id: &'static str,
    label: &'static str,
    icon: gpui::AnyElement,
    t: Theme,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    div()
        .id(SharedString::new_static(id))
        .flex_1()
        .h(px(24.0))
        .rounded(px(5.0))
        .bg(token_rgba(t.button_bg))
        .cursor_pointer()
        .hover(move |s| s.bg(token_rgba(t.button_hover)))
        .flex()
        .items_center()
        .justify_center()
        .gap(px(5.0))
        .child(icon)
        .child(
            div()
                .text_size(px(FOOTER_TEXT_SIZE))
                .line_height(px(13.0))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(token_hsla(t.text_dim))
                .child(label),
        )
        .on_click(on_click)
}
