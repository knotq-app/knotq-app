use gpui::prelude::*;
use gpui::{Context, Entity, Window};
use knotq_model::SchemeId;

use super::{KnotQApp, SchemeEditorMenuState, SchemeSessionState, View};
use knotq_editor::{EditorEvent, SchemeEditor};

impl KnotQApp {
    pub fn ensure_scheme_editor(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<SchemeEditor>> {
        let scheme_id = self.selection.scheme_id?;
        if let Some((existing_id, editor)) = self.scheme_editor.clone() {
            if existing_id == scheme_id {
                return Some(editor);
            }
            self.capture_current_scheme_session(cx);
        }
        let scheme = self.workspace.scheme(scheme_id)?.clone();
        let theme = self.theme();
        let time_format = self.time_format;
        let session = self.scheme_sessions.get(&scheme_id).cloned();
        let should_restore_session = session.is_some() && self.selection.focused_item_id.is_none();
        self.clear_editor_menus();
        let scroll_handle = self.scheme_scroll_handle.clone();
        let editor = cx.new(|cx| {
            SchemeEditor::new(
                scheme_id,
                scheme,
                theme,
                time_format,
                scroll_handle,
                window,
                cx,
            )
        });
        let sub = cx.subscribe_in(&editor, window, Self::on_editor_event);
        self.scheme_editor = Some((scheme_id, editor.clone()));
        self._editor_subscription = Some(sub);
        if let Some(session) = session.filter(|_| should_restore_session) {
            editor.update(cx, |editor, cx| {
                editor.restore_session_state(session.editor, cx);
            });
            self.scheme_scroll_handle.set_offset(session.scroll_offset);
            self.restore_scheme_editor_menu(scheme_id, session.menu, window, cx);
        } else {
            self.scheme_scroll_handle.scroll_to_bottom();
        }
        self.scheme_scroll_initialized_for = Some(scheme_id);
        Some(editor)
    }

    pub fn focus_current_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_date_popover();
        self.close_repeat_popover();
        if self.selection.view == View::DailyQueue {
            if let Some(editor) = self
                .selection
                .scheme_id
                .and_then(|scheme_id| self.workspace.daily_queue_date_for_scheme(scheme_id))
                .and_then(|date| self.ensure_daily_queue_editor(date, window, cx))
            {
                editor.update(cx, |ed, cx| ed.focus(window, cx));
            } else {
                self.editor_focus_handle.focus(window);
            }
            cx.notify();
            return;
        }
        if let Some(editor) = self.ensure_scheme_editor(window, cx) {
            editor.update(cx, |ed, cx| ed.focus(window, cx));
        } else {
            self.editor_focus_handle.focus(window);
        }
        cx.notify();
    }

    pub(crate) fn on_editor_event(
        &mut self,
        editor: &Entity<SchemeEditor>,
        event: &EditorEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A notice (e.g. a rejected image) is always worth showing, regardless
        // of which editor raised it.
        if let EditorEvent::Notice { title, message } = event.clone() {
            self.notice_modal = Some(super::NoticeModal {
                title,
                message,
                button_label: "OK".to_string(),
            });
            cx.notify();
            return;
        }
        if !self.editor_event_is_from_active_view(editor) {
            return;
        }
        match event.clone() {
            EditorEvent::Command(cmd) => {
                self.apply_editor_command(cmd, cx);
            }
            EditorEvent::OpenDatePicker {
                scheme_id,
                item_id,
                kind,
                anchor,
            } => {
                self.editor_context_menu = None;
                self.open_date_popover(scheme_id, item_id, kind, anchor, window, cx);
                cx.notify();
            }
            EditorEvent::OpenRepeatPopover {
                scheme_id,
                item_id,
                anchor,
            } => {
                self.editor_context_menu = None;
                self.open_repeat_popover(scheme_id, item_id, anchor, window, cx);
                cx.notify();
            }
            EditorEvent::OpenLink { scheme_id: _, url } => {
                let _ = crate::app::google_oauth::open_browser(&url);
            }
            EditorEvent::OpenContextMenu {
                scheme_id,
                item_id,
                position,
                date_anchor,
                table,
            } => {
                self.close_date_popover();
                self.close_repeat_popover();
                self.sidebar_context_menu = None;
                self.editor_context_menu = Some(super::EditorContextMenu {
                    scheme_id,
                    item_id,
                    position,
                    date_anchor,
                    table,
                });
                cx.notify();
            }
            EditorEvent::CloseDatePopover => {
                self.close_date_popover();
                self.close_repeat_popover();
                self.editor_context_menu = None;
                cx.notify();
            }
            EditorEvent::Focused { scheme_id } => {
                if self.workspace.is_daily_queue_scheme(scheme_id) {
                    self.selection.view = View::DailyQueue;
                    self.selection.scheme_id = Some(scheme_id);
                }
                self.close_date_popover();
                self.close_repeat_popover();
                self.editor_context_menu = None;
                cx.notify();
            }
            EditorEvent::SelectionChanged { scheme_id } => {
                if self.workspace.is_daily_queue_scheme(scheme_id) {
                    self.selection.view = View::DailyQueue;
                    self.selection.scheme_id = Some(scheme_id);
                }
                // Broadcast the local caret as presence (no-op when ws is down).
                self.send_local_presence(scheme_id, editor, cx);
                cx.notify();
            }
            EditorEvent::Notice { .. } => {}
        }
    }

    fn editor_event_is_from_active_view(&self, editor: &Entity<SchemeEditor>) -> bool {
        match self.selection.view {
            View::Scheme => self
                .scheme_editor
                .as_ref()
                .is_some_and(|(_, active)| active == editor),
            View::DailyQueue => self
                .daily_queue_editors
                .values()
                .any(|active| active == editor),
            View::Union | View::Settings => false,
        }
    }

    pub(crate) fn active_scheme_editor_menu_state(
        &self,
        scheme_id: SchemeId,
    ) -> Option<SchemeEditorMenuState> {
        if let Some(popup) = self
            .date_popover
            .as_ref()
            .filter(|popup| popup.scheme_id == scheme_id)
        {
            return Some(SchemeEditorMenuState::Date {
                item_id: popup.item_id,
                kind: popup.kind,
                anchor: popup.anchor,
            });
        }
        if let Some(popup) = self
            .repeat_popover
            .as_ref()
            .filter(|popup| popup.scheme_id == scheme_id)
        {
            return Some(SchemeEditorMenuState::Repeat {
                item_id: popup.item_id,
                anchor: popup.anchor,
            });
        }
        self.editor_context_menu
            .as_ref()
            .filter(|menu| menu.scheme_id == scheme_id)
            .map(|menu| SchemeEditorMenuState::Context {
                item_id: menu.item_id,
                position: menu.position,
                date_anchor: menu.date_anchor,
                table: menu.table,
            })
    }

    pub(crate) fn capture_current_scheme_session(&mut self, cx: &mut Context<Self>) {
        let Some((scheme_id, editor)) = self.scheme_editor.clone() else {
            return;
        };
        if self.workspace.scheme(scheme_id).is_none() {
            self.scheme_sessions.remove(&scheme_id);
            return;
        }
        let session = SchemeSessionState {
            editor: editor.read(cx).session_state(),
            scroll_offset: self.scheme_scroll_handle.offset(),
            menu: self.active_scheme_editor_menu_state(scheme_id),
        };
        self.scheme_sessions.insert(scheme_id, session);
    }

    pub(crate) fn clear_editor_menus(&mut self) {
        self.close_date_popover();
        self.close_repeat_popover();
        self.editor_context_menu = None;
    }

    pub(crate) fn restore_scheme_editor_menu(
        &mut self,
        scheme_id: SchemeId,
        menu: Option<SchemeEditorMenuState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(menu) = menu else {
            return;
        };
        match menu {
            SchemeEditorMenuState::Date {
                item_id,
                kind,
                anchor,
            } => {
                if self.scheme_item_exists(scheme_id, item_id) {
                    self.open_date_popover(scheme_id, item_id, kind, anchor, window, cx);
                }
            }
            SchemeEditorMenuState::Repeat { item_id, anchor } => {
                if self.scheme_item_exists(scheme_id, item_id) {
                    self.open_repeat_popover(scheme_id, item_id, anchor, window, cx);
                }
            }
            SchemeEditorMenuState::Context {
                item_id,
                position,
                date_anchor,
                table,
            } => {
                let item_exists = self.scheme_item_exists(scheme_id, item_id);
                let table_exists = table
                    .is_some_and(|table| self.scheme_item_exists(scheme_id, table.table_item_id));
                if item_exists || table_exists {
                    self.editor_context_menu = Some(super::EditorContextMenu {
                        scheme_id,
                        item_id,
                        position,
                        date_anchor,
                        table,
                    });
                }
            }
        }
    }
}
