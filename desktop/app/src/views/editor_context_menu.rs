use gpui::prelude::*;
use gpui::{
    div, px, ClickEvent, Context, IntoElement, MouseButton, MouseDownEvent, SharedString, Window,
};
use knotq_commands::{Command, DateKind};
use knotq_model::ItemMarker;

use crate::app::{EditorContextMenu, KnotQApp};
use crate::theme_gpui::{token_hsla, token_rgba, Theme, FONT_UI};
use knotq_ui::{clamped_popover_left, popover_top_biased_below};

impl KnotQApp {
    fn close_editor_context_menu(&mut self, cx: &mut Context<Self>) {
        self.editor_context_menu = None;
        cx.notify();
    }

    pub fn render_editor_context_menu(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let menu = self.editor_context_menu.clone()?;
        let scheme = self.workspace.scheme(menu.scheme_id)?;
        if scheme.is_read_only() {
            return None;
        }
        let item = scheme.item(menu.item_id)?;
        let t = self.theme();

        let mut items: Vec<gpui::AnyElement> = Vec::new();
        if item.marker == ItemMarker::Checkbox {
            push_date_items(
                &mut items,
                DateMenuItem {
                    id_prefix: "start",
                    label: "Start",
                    kind: DateKind::Start,
                    exists: item.start.is_some(),
                },
                &menu,
                t,
                cx,
            );
            push_date_items(
                &mut items,
                DateMenuItem {
                    id_prefix: "end",
                    label: "End",
                    kind: DateKind::End,
                    exists: item.end.is_some(),
                },
                &menu,
                t,
                cx,
            );
        }

        if items.is_empty() {
            return None;
        }
        let viewport_width = px(f32::from(window.viewport_size().width));
        let viewport_height = px(f32::from(window.viewport_size().height));
        let menu_width = px(140.0);
        let menu_height = px(items.len() as f32 * 29.0 + 8.0);
        let menu_left = clamped_popover_left(menu.position.x, menu_width, viewport_width);
        let menu_top = popover_top_biased_below(menu.position.y, menu_height, viewport_height);

        Some(
            div()
                .id("editor-context-menu-scrim")
                .absolute()
                .inset_0()
                .bg(token_rgba(0x00000000))
                .occlude()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                        this.close_editor_context_menu(cx);
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, _: &MouseDownEvent, _window, cx| {
                        this.close_editor_context_menu(cx);
                    }),
                )
                .child(
                    div()
                        .id("editor-context-menu")
                        .absolute()
                        .left(menu_left)
                        .top(menu_top)
                        .occlude()
                        .min_w(px(140.0))
                        .p(px(4.0))
                        .rounded(px(7.0))
                        .bg(token_hsla(t.bg_sidebar))
                        .border_1()
                        .border_color(token_rgba(t.border_overlay))
                        .flex()
                        .flex_col()
                        .gap(px(1.0))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                        .children(items),
                )
                .into_any_element(),
        )
    }
}

#[derive(Clone, Copy)]
struct DateMenuItem {
    id_prefix: &'static str,
    label: &'static str,
    kind: DateKind,
    exists: bool,
}

fn push_date_items(
    items: &mut Vec<gpui::AnyElement>,
    spec: DateMenuItem,
    menu: &EditorContextMenu,
    t: Theme,
    cx: &mut Context<KnotQApp>,
) {
    let DateMenuItem {
        id_prefix,
        label,
        kind,
        exists,
    } = spec;
    let scheme_id = menu.scheme_id;
    let item_id = menu.item_id;
    let anchor = menu.date_anchor;
    let action_label = if exists {
        format!("Edit {label}")
    } else {
        format!("Add {label}")
    };
    items.push(editor_context_item(
        SharedString::from(format!("editor-menu-{id_prefix}-edit")),
        action_label,
        t,
        cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.editor_context_menu = None;
            this.open_date_popover(scheme_id, item_id, kind, anchor, window, cx);
            cx.stop_propagation();
            cx.notify();
        }),
    ));

    if exists {
        items.push(editor_context_item(
            SharedString::from(format!("editor-menu-{id_prefix}-remove")),
            format!("Remove {label}"),
            t,
            cx.listener(move |this, _: &ClickEvent, _window, cx| {
                this.editor_context_menu = None;
                this.apply(
                    Command::SetItemDate {
                        scheme: scheme_id,
                        item: item_id,
                        kind,
                        date: None,
                    },
                    cx,
                );
                cx.stop_propagation();
                cx.notify();
            }),
        ));
    }
}

fn editor_context_item(
    id: SharedString,
    label: String,
    t: Theme,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> gpui::AnyElement {
    div()
        .id(id)
        .h(px(28.0))
        .px(px(10.0))
        .rounded(px(5.0))
        .flex()
        .items_center()
        .text_size(px(13.0))
        .font_family(FONT_UI)
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(token_hsla(t.text_primary))
        .cursor_pointer()
        .hover(move |s| s.bg(token_rgba(t.row_hover_strong)))
        .on_click(on_click)
        .child(label)
        .into_any_element()
}
