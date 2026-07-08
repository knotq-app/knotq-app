use gpui::prelude::*;
use gpui::{
    div, px, ClickEvent, Context, IntoElement, MouseButton, MouseDownEvent, SharedString, Window,
};
use knotq_commands::{Command, DateKind};
use knotq_editor::{TableContext, TableStructureAction};
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
        let item = scheme.item(menu.item_id);
        let t = self.theme();

        let mut items: Vec<gpui::AnyElement> = Vec::new();
        if let Some(item) = item.filter(|item| item.marker == ItemMarker::Checkbox) {
            push_date_items(
                &mut items,
                DateMenuItem {
                    id_prefix: "start",
                    label: "editor.context.date_start",
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
                    label: "editor.context.date_end",
                    kind: DateKind::End,
                    exists: item.end.is_some(),
                },
                &menu,
                t,
                cx,
            );
        }

        if let Some(table) = menu.table {
            if !items.is_empty() {
                items.push(editor_context_separator(t));
            }
            push_table_items(&mut items, &menu, table, t, cx);
        }

        if items.is_empty() {
            return None;
        }
        let viewport_width = px(f32::from(window.viewport_size().width));
        let viewport_height = px(f32::from(window.viewport_size().height));
        let menu_width = if menu.table.is_some() {
            px(184.0)
        } else {
            px(140.0)
        };
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
                        .min_w(menu_width)
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

fn push_table_items(
    items: &mut Vec<gpui::AnyElement>,
    menu: &EditorContextMenu,
    table: TableContext,
    t: Theme,
    cx: &mut Context<KnotQApp>,
) {
    let mut pushed_row_items = false;
    if let Some(row) = table.row {
        push_table_action(
            items,
            "row-before",
            "editor.context.insert_row_before",
            table.table_item_id,
            TableStructureAction::InsertRowBefore(row),
            menu,
            t,
            cx,
        );
        push_table_action(
            items,
            "row-after",
            "editor.context.insert_row_after",
            table.table_item_id,
            TableStructureAction::InsertRowAfter(row),
            menu,
            t,
            cx,
        );
        if table.row_count > 1 {
            push_table_action(
                items,
                "row-delete",
                "editor.context.delete_row",
                table.table_item_id,
                TableStructureAction::DeleteRow(row),
                menu,
                t,
                cx,
            );
        }
        pushed_row_items = true;
    }

    if pushed_row_items && table.column.is_some() {
        items.push(editor_context_separator(t));
    }

    if let Some(column) = table.column {
        push_table_action(
            items,
            "column-before",
            "editor.context.insert_column_before",
            table.table_item_id,
            TableStructureAction::InsertColumnBefore(column),
            menu,
            t,
            cx,
        );
        push_table_action(
            items,
            "column-after",
            "editor.context.insert_column_after",
            table.table_item_id,
            TableStructureAction::InsertColumnAfter(column),
            menu,
            t,
            cx,
        );
        if table.column_count > 1 {
            push_table_action(
                items,
                "column-delete",
                "editor.context.delete_column",
                table.table_item_id,
                TableStructureAction::DeleteColumn(column),
                menu,
                t,
                cx,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn push_table_action(
    items: &mut Vec<gpui::AnyElement>,
    id_suffix: &'static str,
    label: &'static str,
    table_item_id: knotq_model::ItemId,
    action: TableStructureAction,
    menu: &EditorContextMenu,
    t: Theme,
    cx: &mut Context<KnotQApp>,
) {
    let scheme_id = menu.scheme_id;
    items.push(editor_context_item(
        SharedString::from(format!("editor-menu-table-{id_suffix}")),
        knotq_l10n::t(label).to_string(),
        t,
        cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.editor_context_menu = None;
            let editor = if let Some((active_scheme_id, editor)) = this.scheme_editor.as_ref() {
                (*active_scheme_id == scheme_id).then_some(editor.clone())
            } else {
                None
            }
            .or_else(|| {
                this.workspace
                    .daily_queue_date_for_scheme(scheme_id)
                    .and_then(|date| this.daily_queue_editors.get(&date).cloned())
            });

            if let Some(editor) = editor {
                editor.update(cx, |editor, cx| {
                    editor.apply_table_structure_action(table_item_id, action, window, cx);
                });
            }
            cx.stop_propagation();
            cx.notify();
        }),
    ));
}

#[derive(Clone, Copy)]
struct DateMenuItem {
    id_prefix: &'static str,
    /// l10n key for the date kind's display name (e.g. "Start"/"End").
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
    let label_text = knotq_l10n::t(label);
    let action_label = if exists {
        knotq_l10n::t_with("editor.context.edit_label", &[("label", label_text)])
    } else {
        knotq_l10n::t_with("editor.context.add_label", &[("label", label_text)])
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
            knotq_l10n::t_with("editor.context.remove_label", &[("label", label_text)]),
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

fn editor_context_separator(t: Theme) -> gpui::AnyElement {
    div()
        .h(px(1.0))
        .mx(px(6.0))
        .my(px(3.0))
        .bg(token_rgba(t.border_overlay))
        .into_any_element()
}
