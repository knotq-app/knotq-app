use super::*;
use crate::app::GoogleOAuthStatus;
use knotq_model::{CalendarProvider, ItemMarker, Scheme, SchemeId, SchemeSource};

impl KnotQApp {
    pub(crate) fn render_scheme_toolbar(
        &mut self,
        scheme: &Scheme,
        editor: Entity<SchemeEditor>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let t = self.theme();
        let c = t;
        if scheme.is_read_only() {
            let account_label = self
                .imported_calendar_account_label(scheme)
                .unwrap_or_else(|| "Imported calendar".to_string());
            let google_scheme_id = match &scheme.source {
                SchemeSource::ImportedCalendar(source)
                    if source.provider == CalendarProvider::Google =>
                {
                    Some(scheme.id)
                }
                _ => None,
            };
            let syncing = matches!(self.google_oauth_status, GoogleOAuthStatus::InProgress);
            return read_only_toolbar(account_label, google_scheme_id, syncing, c, cx);
        }

        let state = editor.read(cx).toolbar_state();
        let marker_button = |id: &'static str,
                             marker: ItemMarker,
                             glyph: ToolbarGlyph,
                             tooltip: &'static str,
                             editor: Entity<SchemeEditor>,
                             cx: &mut Context<Self>| {
            let active = state.marker == marker;
            toolbar_glyph_button(
                id,
                active,
                glyph,
                c,
                tooltip,
                editor.clone(),
                cx.listener(move |_this, _: &ClickEvent, _window, cx| {
                    editor.update(cx, |editor, cx| editor.set_marker_for_selection(marker, cx));
                }),
            )
        };

        let bold_editor = editor.clone();
        let italic_editor = editor.clone();
        let heading_editor = editor.clone();
        let indent_editor = editor.clone();
        let unindent_editor = editor.clone();
        let start_editor = editor.clone();
        let end_editor = editor.clone();
        let repeat_editor = editor.clone();

        div()
            .id("scheme-toolbar")
            .absolute()
            .bottom(px(14.0))
            .left_0()
            .right_0()
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .px(px(12.0))
            .child(
                div()
                    .id("scheme-format-palette")
                    .h(px(29.0))
                    .flex()
                    .items_center()
                    .gap(px(2.0))
                    .px(px(4.0))
                    .rounded(px(9.0))
                    .bg(token_rgba(c.toolbar_chip_bg))
                    .border_1()
                    .border_color(token_rgba(c.toolbar_chip_border))
                    .child(marker_button(
                        "scheme-toolbar-blank",
                        ItemMarker::Blank,
                        ToolbarGlyph::Plain,
                        "plain line (⌘1)",
                        editor.clone(),
                        cx,
                    ))
                    .child(marker_button(
                        "scheme-toolbar-checkbox",
                        ItemMarker::Checkbox,
                        ToolbarGlyph::Checkbox,
                        "checkbox line (⌘2)",
                        editor.clone(),
                        cx,
                    ))
                    .child(marker_button(
                        "scheme-toolbar-bullet",
                        ItemMarker::Bullet,
                        ToolbarGlyph::Bullet,
                        "bullet line (⌘3)",
                        editor.clone(),
                        cx,
                    ))
                    .child(marker_button(
                        "scheme-toolbar-numbered",
                        ItemMarker::Numbered,
                        ToolbarGlyph::Numbered,
                        "numbered line (⌘4)",
                        editor.clone(),
                        cx,
                    ))
                    .child(toolbar_separator(c.toolbar_chip_separator))
                    .child(toolbar_date_button(
                        "scheme-toolbar-start",
                        "start",
                        state.has_start,
                        c,
                        "start date (⌘S)",
                        editor.clone(),
                        cx.listener(move |_this, _: &ClickEvent, _window, cx| {
                            start_editor
                                .update(cx, |editor, cx| editor.toggle_start_date_from_toolbar(cx));
                        }),
                    ))
                    .child(toolbar_date_button(
                        "scheme-toolbar-end",
                        "end",
                        state.has_end,
                        c,
                        "end date (⌘E)",
                        editor.clone(),
                        cx.listener(move |_this, _: &ClickEvent, _window, cx| {
                            end_editor
                                .update(cx, |editor, cx| editor.toggle_end_date_from_toolbar(cx));
                        }),
                    ))
                    .child(toolbar_date_button(
                        "scheme-toolbar-repeat",
                        "repeat",
                        state.has_repeat,
                        c,
                        "repeat (⌘R)",
                        editor.clone(),
                        cx.listener(move |_this, _: &ClickEvent, _window, cx| {
                            repeat_editor
                                .update(cx, |editor, cx| editor.toggle_repeat_from_toolbar(cx));
                        }),
                    ))
                    .child(toolbar_separator(c.toolbar_chip_separator))
                    .child(toolbar_bold_button(
                        state.bold,
                        c,
                        "bold (⌘B)",
                        editor.clone(),
                        cx.listener(move |_this, _: &ClickEvent, _window, cx| {
                            bold_editor
                                .update(cx, |editor, cx| editor.toggle_bold_from_toolbar(cx));
                        }),
                    ))
                    .child(toolbar_italic_button(
                        state.italic,
                        c,
                        "italic (⌘I)",
                        editor.clone(),
                        cx.listener(move |_this, _: &ClickEvent, _window, cx| {
                            italic_editor
                                .update(cx, |editor, cx| editor.toggle_italic_from_toolbar(cx));
                        }),
                    ))
                    .child(toolbar_glyph_button(
                        "scheme-toolbar-heading",
                        state.heading,
                        ToolbarGlyph::Heading,
                        c,
                        "heading (⌘J)",
                        editor.clone(),
                        cx.listener(move |_this, _: &ClickEvent, _window, cx| {
                            heading_editor
                                .update(cx, |editor, cx| editor.toggle_heading_from_toolbar(cx));
                        }),
                    ))
                    .child(toolbar_separator(c.toolbar_chip_separator))
                    .child(toolbar_glyph_button(
                        "scheme-toolbar-unindent",
                        false,
                        ToolbarGlyph::Unindent,
                        c,
                        "unindent (⇧Tab)",
                        editor.clone(),
                        cx.listener(move |_this, _: &ClickEvent, _window, cx| {
                            unindent_editor
                                .update(cx, |editor, cx| editor.indent_from_toolbar(-1, cx));
                        }),
                    ))
                    .child(toolbar_glyph_button(
                        "scheme-toolbar-indent",
                        false,
                        ToolbarGlyph::Indent,
                        c,
                        "indent (Tab)",
                        editor.clone(),
                        cx.listener(move |_this, _: &ClickEvent, _window, cx| {
                            indent_editor
                                .update(cx, |editor, cx| editor.indent_from_toolbar(1, cx));
                        }),
                    )),
            )
            .into_any_element()
    }
}

fn read_only_toolbar(
    account_label: String,
    google_scheme_id: Option<SchemeId>,
    syncing: bool,
    c: Theme,
    cx: &mut Context<KnotQApp>,
) -> gpui::AnyElement {
    let read_only_text = token_hsla(c.toolbar_chip_selected_text);
    let muted_text = token_hsla(c.toolbar_chip_muted);
    div()
        .id("scheme-toolbar")
        .absolute()
        .bottom(px(14.0))
        .left_0()
        .right_0()
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .px(px(12.0))
        .child(
            div()
                .id("scheme-read-only-palette")
                .h(px(29.0))
                .max_w(px(360.0))
                .flex()
                .items_center()
                .gap(px(7.0))
                .px(px(10.0))
                .rounded(px(9.0))
                .bg(token_rgba(c.toolbar_chip_bg))
                .border_1()
                .border_color(token_rgba(c.toolbar_chip_border))
                .font_family(FONT_UI)
                .text_size(px(11.0))
                .line_height(px(14.0))
                .child(
                    div()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(read_only_text)
                        .child("Read only"),
                )
                .child(toolbar_separator(c.toolbar_chip_separator))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(read_only_text)
                        .child(account_label),
                )
                .when_some(google_scheme_id, |palette, scheme_id| {
                    palette
                        .child(toolbar_separator(c.toolbar_chip_separator))
                        .child(read_only_refresh_button(
                            scheme_id,
                            syncing,
                            read_only_text,
                            muted_text,
                            cx,
                        ))
                }),
        )
        .into_any_element()
}

fn read_only_refresh_button(
    scheme_id: SchemeId,
    syncing: bool,
    text_color: gpui::Hsla,
    muted_text: gpui::Hsla,
    cx: &mut Context<KnotQApp>,
) -> gpui::AnyElement {
    div()
        .id("scheme-toolbar-google-refresh")
        .w(px(24.0))
        .h(px(23.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.0))
        .opacity(if syncing { 0.55 } else { 1.0 })
        .when(!syncing, |button| {
            button
                .cursor_pointer()
                .hover(|s| s.bg(token_rgba(0x00000016)))
                .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    this.start_google_calendar_scheme_refresh(scheme_id, cx);
                    cx.stop_propagation();
                }))
        })
        .tooltip(move |window, cx| {
            Tooltip::new(if syncing {
                "refreshing Google Calendar"
            } else {
                "refresh Google Calendar"
            })
            .build(window, cx)
        })
        .child(
            Icon::new(if syncing {
                IconName::LoaderCircle
            } else {
                IconName::Redo2
            })
            .with_size(px(13.0))
            .text_color(if syncing { muted_text } else { text_color }),
        )
        .into_any_element()
}
