use std::time::Instant;

use gpui::Context;
use knotq_commands::{
    filter_recurring_occurrence_toggles, Command, CommandReceipt, WorkspaceCommandExt,
};
use knotq_model::{Item, ItemId, ItemKind, SchemeId, Workspace};

use super::{
    calendar_toggle_keys, editor_undo_key, recurrence_undo_key, should_coalesce_editor_undo,
    should_coalesce_recurrence_undo, EditorUndoGroup, EditorUndoKey, KnotQApp, UndoNavigationEntry,
    UndoNavigationSnapshot, View, UNDO_DEPTH,
};

impl KnotQApp {
    /// Apply a command, push its inverse onto the undo stack, and mark dirty.
    pub fn apply(&mut self, cmd: Command, cx: &mut Context<Self>) -> Option<CommandReceipt> {
        self.editor_undo_group = None;
        let Some(cmd) = filter_recurring_occurrence_toggles(cmd, &self.workspace) else {
            self.recurrence_undo_group = None;
            return None;
        };
        let recurrence_key = recurrence_undo_key(&cmd);
        let coalesce_recurrence = should_coalesce_recurrence_undo(
            recurrence_key,
            self.recurrence_undo_group,
            self.active_repeat_popover_undo_key(),
        );
        let nav_before = self.undo_navigation_snapshot();
        let toggled = calendar_toggle_keys(&cmd);
        let service_signals = service_signals_for_command(&cmd, &self.workspace);
        self.clear_deleted_item_notifications(&cmd);
        self.state.mark_dirty_from_command(&cmd);
        match self.workspace.apply(cmd) {
            Ok(receipt) => {
                self.sync_retained_completed_calendar_items(&toggled);
                self.recurrence_undo_group = recurrence_key.map(|key| EditorUndoGroup {
                    key,
                    last_edit: Instant::now(),
                });
                self.redo_stack.clear();
                self.reconcile_workspace_ui_state();
                let nav_after = self.undo_navigation_snapshot();
                if !coalesce_recurrence {
                    self.push_undo(
                        receipt.inverse.clone(),
                        UndoNavigationEntry {
                            before: nav_before,
                            after: nav_after,
                        },
                    );
                }
                self.redo_navigation_stack.clear();
                self.signal_workspace_services(service_signals);
                cx.notify();
                Some(receipt)
            }
            Err(err) => {
                eprintln!("command failed: {err}");
                None
            }
        }
    }

    /// Apply a command as part of an existing undoable user action.
    pub(crate) fn apply_without_pushing_undo(
        &mut self,
        cmd: Command,
        cx: &mut Context<Self>,
    ) -> Option<CommandReceipt> {
        self.editor_undo_group = None;
        self.recurrence_undo_group = None;
        let Some(cmd) = filter_recurring_occurrence_toggles(cmd, &self.workspace) else {
            return None;
        };
        let toggled = calendar_toggle_keys(&cmd);
        let service_signals = service_signals_for_command(&cmd, &self.workspace);
        self.clear_deleted_item_notifications(&cmd);
        self.state.mark_dirty_from_command(&cmd);
        match self.workspace.apply(cmd) {
            Ok(receipt) => {
                self.sync_retained_completed_calendar_items(&toggled);
                self.redo_stack.clear();
                self.redo_navigation_stack.clear();
                self.reconcile_workspace_ui_state();
                self.signal_workspace_services(service_signals);
                cx.notify();
                Some(receipt)
            }
            Err(err) => {
                eprintln!("command failed: {err}");
                None
            }
        }
    }

    pub(crate) fn retarget_pending_creation_undo(
        &mut self,
        item_id: ItemId,
        target_scheme_id: SchemeId,
    ) {
        if let Some(Command::DeleteItem { scheme, item }) = self.undo_stack.back_mut() {
            if *item == item_id {
                *scheme = target_scheme_id;
            }
        }
    }

    /// Like `apply` but coalesces consecutive text edits on the same item into
    /// a single undo entry when they occur within the grouping window.
    pub(crate) fn apply_editor_command(
        &mut self,
        cmd: Command,
        cx: &mut Context<Self>,
    ) -> Option<CommandReceipt> {
        self.recurrence_undo_group = None;
        let Some(cmd) = filter_recurring_occurrence_toggles(cmd, &self.workspace) else {
            self.editor_undo_group = None;
            return None;
        };
        let now = Instant::now();
        let key = editor_undo_key(&cmd);
        let coalesce = should_coalesce_editor_undo(key, self.editor_undo_group, now);
        let nav_before = self.undo_navigation_snapshot();
        let toggled = calendar_toggle_keys(&cmd);
        let service_signals = service_signals_for_command(&cmd, &self.workspace);
        self.clear_deleted_item_notifications(&cmd);
        self.state.mark_dirty_from_command(&cmd);

        match self.workspace.apply(cmd) {
            Ok(receipt) => {
                self.sync_retained_completed_calendar_items(&toggled);
                self.editor_undo_group = key.map(|key| EditorUndoGroup {
                    key,
                    last_edit: now,
                });
                self.redo_stack.clear();
                self.reconcile_workspace_ui_state();
                let nav_after = self.undo_navigation_snapshot();
                if !coalesce {
                    self.push_undo(
                        receipt.inverse.clone(),
                        UndoNavigationEntry {
                            before: nav_before,
                            after: nav_after,
                        },
                    );
                }
                self.redo_navigation_stack.clear();
                self.signal_workspace_services(service_signals);
                cx.notify();
                Some(receipt)
            }
            Err(err) => {
                self.editor_undo_group = None;
                eprintln!("editor command failed: {err}");
                None
            }
        }
    }

    pub(crate) fn item_allows_occurrence_toggle(
        &self,
        scheme_id: SchemeId,
        item_id: ItemId,
        occurrence: &knotq_model::OccurrenceId,
    ) -> bool {
        self.workspace
            .scheme(scheme_id)
            .and_then(|scheme| scheme.item(item_id))
            .is_some_and(|item| item.repeats.is_none() || !occurrence.is_single())
    }

    fn clear_deleted_item_notifications(&self, cmd: &Command) {
        let mut deleted = Vec::new();
        collect_deleted_items(cmd, &mut deleted);
        deleted.sort_unstable_by_key(|(scheme, item)| (scheme.0, item.0));
        deleted.dedup();
        for (scheme, item) in deleted {
            crate::notifications::clear_item_notifications(
                &self.workspace,
                self.notification_defaults,
                scheme,
                item,
            );
        }
    }

    fn active_repeat_popover_undo_key(&self) -> Option<EditorUndoKey> {
        self.repeat_popover.as_ref().map(|popup| EditorUndoKey {
            scheme_id: popup.scheme_id,
            item_id: popup.item_id,
        })
    }

    fn undo_navigation_snapshot(&self) -> UndoNavigationSnapshot {
        UndoNavigationSnapshot {
            selection: self.selection.clone(),
            week_offset: self.week_offset,
            month_offset: self.month_offset,
        }
    }

    fn restore_undo_navigation_snapshot(
        &mut self,
        snapshot: &UndoNavigationSnapshot,
        _cx: &mut Context<Self>,
    ) {
        let view_changed = self.selection.view != snapshot.selection.view
            || self.selection.scheme_id != snapshot.selection.scheme_id;
        if view_changed {
            self.close_date_popover();
            self.close_repeat_popover();
        }

        self.week_offset = snapshot.week_offset;
        self.month_offset = snapshot.month_offset;

        let mut selection = snapshot.selection.clone();
        match selection.view {
            View::Scheme => {
                let Some(scheme_id) = selection.scheme_id else {
                    self.open_union();
                    return;
                };
                let Some(scheme) = self.workspace.scheme(scheme_id) else {
                    self.open_union();
                    return;
                };
                if selection
                    .focused_item_id
                    .is_some_and(|item_id| scheme.item(item_id).is_none())
                {
                    selection.focused_item_id = None;
                }
                self.selection = selection;
            }
            View::DailyQueue => {
                if selection
                    .scheme_id
                    .is_some_and(|scheme_id| self.workspace.scheme(scheme_id).is_none())
                {
                    selection.scheme_id = self
                        .workspace
                        .daily_queue_scheme_id(self.daily_queue_today)
                        .filter(|scheme_id| self.workspace.scheme(*scheme_id).is_some());
                }
                if let Some(scheme_id) = selection.scheme_id {
                    if selection
                        .focused_item_id
                        .is_some_and(|item_id| !self.scheme_item_exists(scheme_id, item_id))
                    {
                        selection.focused_item_id = None;
                    }
                }
                self.selection = selection;
            }
            View::Union | View::Settings => {
                self.selection = selection;
            }
        }

        self.dismiss_event_popup_if_hidden_context();
    }

    pub(super) fn push_undo(&mut self, inverse: Command, navigation: UndoNavigationEntry) {
        self.undo_stack.push_back(inverse);
        self.undo_navigation_stack.push_back(navigation);
        while self.undo_stack.len() > UNDO_DEPTH {
            self.undo_stack.pop_front();
            self.undo_navigation_stack.pop_front();
        }
        while self.undo_navigation_stack.len() > self.undo_stack.len() {
            self.undo_navigation_stack.pop_front();
        }
    }

    pub fn undo(&mut self, cx: &mut Context<Self>) {
        self.editor_undo_group = None;
        self.recurrence_undo_group = None;
        if let Some(inv) = self.undo_stack.pop_back() {
            let navigation = self.undo_navigation_stack.pop_back();
            let toggled = calendar_toggle_keys(&inv);
            let service_signals = service_signals_for_command(&inv, &self.workspace);
            self.state.mark_dirty_from_command(&inv);
            if let Ok(receipt) = self.workspace.apply(inv) {
                self.sync_retained_completed_calendar_items(&toggled);
                self.redo_stack.push_back(receipt.inverse);
                if let Some(navigation) = navigation.as_ref() {
                    self.redo_navigation_stack.push_back(navigation.clone());
                }
                self.reconcile_workspace_ui_state();
                if let Some(navigation) = navigation {
                    self.restore_undo_navigation_snapshot(&navigation.before, cx);
                }
                self.signal_workspace_services(service_signals);
                cx.notify();
            }
        }
    }

    pub fn redo(&mut self, cx: &mut Context<Self>) {
        self.editor_undo_group = None;
        self.recurrence_undo_group = None;
        if let Some(inv) = self.redo_stack.pop_back() {
            let navigation = self.redo_navigation_stack.pop_back();
            let toggled = calendar_toggle_keys(&inv);
            let service_signals = service_signals_for_command(&inv, &self.workspace);
            self.state.mark_dirty_from_command(&inv);
            if let Ok(receipt) = self.workspace.apply(inv) {
                self.sync_retained_completed_calendar_items(&toggled);
                if let Some(navigation) = navigation.as_ref() {
                    self.push_undo(receipt.inverse, navigation.clone());
                } else {
                    self.undo_stack.push_back(receipt.inverse);
                }
                self.reconcile_workspace_ui_state();
                if let Some(navigation) = navigation {
                    self.restore_undo_navigation_snapshot(&navigation.after, cx);
                }
                self.signal_workspace_services(service_signals);
                cx.notify();
            }
        }
    }

    fn signal_workspace_services(&self, signals: WorkspaceServiceSignals) {
        self.service_bus.signal_save();
        match signals.notifications {
            NotificationServiceSignal::None => {}
            NotificationServiceSignal::Recompute => self.service_bus.signal_notifications(),
            NotificationServiceSignal::RefreshItems(items) => {
                for (scheme_id, item_id) in items {
                    if let Some(item) = workspace_item(&self.workspace, scheme_id, item_id) {
                        self.service_bus.signal_item_notifications(
                            scheme_id,
                            item.clone(),
                            self.notification_defaults,
                        );
                    }
                }
            }
        }
        if signals.timeline {
            self.service_bus.signal_timeline();
        }
    }
}

fn collect_deleted_items(cmd: &Command, out: &mut Vec<(SchemeId, ItemId)>) {
    match cmd {
        Command::DeleteItem { scheme, item } => out.push((*scheme, *item)),
        Command::Batch(cmds) => {
            for cmd in cmds {
                collect_deleted_items(cmd, out);
            }
        }
        _ => {}
    }
}

#[derive(Clone)]
struct WorkspaceServiceSignals {
    notifications: NotificationServiceSignal,
    timeline: bool,
}

#[derive(Clone)]
enum NotificationServiceSignal {
    None,
    Recompute,
    RefreshItems(Vec<(SchemeId, ItemId)>),
}

fn service_signals_for_command(cmd: &Command, workspace: &Workspace) -> WorkspaceServiceSignals {
    match cmd {
        Command::Batch(commands) => commands
            .iter()
            .map(|cmd| service_signals_for_command(cmd, workspace))
            .fold(
                WorkspaceServiceSignals {
                    notifications: NotificationServiceSignal::None,
                    timeline: false,
                },
                |acc, next| WorkspaceServiceSignals {
                    notifications: combine_notification_signals(
                        acc.notifications,
                        next.notifications,
                    ),
                    timeline: acc.timeline || next.timeline,
                },
            ),
        Command::UpdateItemText { scheme, item, .. } => WorkspaceServiceSignals {
            notifications: if workspace_item_may_schedule_notifications(workspace, *scheme, *item) {
                NotificationServiceSignal::RefreshItems(vec![(*scheme, *item)])
            } else {
                NotificationServiceSignal::None
            },
            timeline: false,
        },
        Command::SetItemIndent { .. }
        | Command::SetItemPriority { .. }
        | Command::RenameScheme { .. }
        | Command::SetSchemeColor { .. }
        | Command::SetFolderExpanded { .. }
        | Command::RenameFolder { .. }
        | Command::MoveNode { .. } => WorkspaceServiceSignals {
            notifications: NotificationServiceSignal::None,
            timeline: false,
        },
        Command::SetItemMarker { scheme, item, .. } => {
            let notifications =
                workspace_item_may_schedule_notifications(workspace, *scheme, *item);
            let timeline = workspace_item_may_complete_in_timeline(workspace, *scheme, *item);
            WorkspaceServiceSignals {
                notifications: if notifications {
                    NotificationServiceSignal::Recompute
                } else {
                    NotificationServiceSignal::None
                },
                timeline,
            }
        }
        Command::CreateFolder { .. } | Command::CreateScheme { .. } => WorkspaceServiceSignals {
            notifications: NotificationServiceSignal::None,
            timeline: false,
        },
        Command::SetOccurrenceNotificationOffset { .. } => WorkspaceServiceSignals {
            notifications: NotificationServiceSignal::Recompute,
            timeline: false,
        },
        Command::InsertItem { item, .. } => item_service_signals(item),
        Command::ReplaceItem { scheme, item } => {
            let before = workspace_item(workspace, *scheme, item.id);
            let notifications = item_may_schedule_notifications(item)
                || before.is_some_and(item_may_schedule_notifications);
            WorkspaceServiceSignals {
                notifications: if notifications {
                    NotificationServiceSignal::Recompute
                } else {
                    NotificationServiceSignal::None
                },
                timeline: item_may_complete_in_timeline(item)
                    || before.is_some_and(item_may_complete_in_timeline),
            }
        }
        Command::DeleteItem { scheme, item } => WorkspaceServiceSignals {
            notifications: if workspace_item_may_schedule_notifications(workspace, *scheme, *item) {
                NotificationServiceSignal::Recompute
            } else {
                NotificationServiceSignal::None
            },
            timeline: workspace_item_may_complete_in_timeline(workspace, *scheme, *item),
        },
        Command::SetItemDate { .. }
        | Command::SetItemRecurrence { .. }
        | Command::ToggleOccurrence { .. }
        | Command::RestoreScheme { .. }
        | Command::RestoreDeletedScheme { .. }
        | Command::SetSchemeGsync { .. }
        | Command::SetSchemeSource { .. }
        | Command::DeleteScheme { .. }
        | Command::PermanentlyDeleteScheme { .. }
        | Command::RestoreFolder { .. }
        | Command::DeleteFolder { .. } => WorkspaceServiceSignals {
            notifications: NotificationServiceSignal::Recompute,
            timeline: true,
        },
        Command::ReorderItem { .. } => WorkspaceServiceSignals {
            notifications: NotificationServiceSignal::None,
            timeline: false,
        },
    }
}

fn item_service_signals(item: &Item) -> WorkspaceServiceSignals {
    WorkspaceServiceSignals {
        notifications: if item_may_schedule_notifications(item) {
            NotificationServiceSignal::Recompute
        } else {
            NotificationServiceSignal::None
        },
        timeline: item_may_complete_in_timeline(item),
    }
}

fn combine_notification_signals(
    left: NotificationServiceSignal,
    right: NotificationServiceSignal,
) -> NotificationServiceSignal {
    match (left, right) {
        (NotificationServiceSignal::Recompute, _) | (_, NotificationServiceSignal::Recompute) => {
            NotificationServiceSignal::Recompute
        }
        (
            NotificationServiceSignal::RefreshItems(mut left),
            NotificationServiceSignal::RefreshItems(right),
        ) => {
            left.extend(right);
            NotificationServiceSignal::RefreshItems(left)
        }
        (NotificationServiceSignal::RefreshItems(items), NotificationServiceSignal::None)
        | (NotificationServiceSignal::None, NotificationServiceSignal::RefreshItems(items)) => {
            NotificationServiceSignal::RefreshItems(items)
        }
        (NotificationServiceSignal::None, NotificationServiceSignal::None) => {
            NotificationServiceSignal::None
        }
    }
}

fn workspace_item(workspace: &Workspace, scheme_id: SchemeId, item_id: ItemId) -> Option<&Item> {
    workspace
        .scheme(scheme_id)
        .and_then(|scheme| scheme.item(item_id))
}

fn workspace_item_may_schedule_notifications(
    workspace: &Workspace,
    scheme_id: SchemeId,
    item_id: ItemId,
) -> bool {
    workspace_item(workspace, scheme_id, item_id).is_some_and(item_may_schedule_notifications)
}

fn workspace_item_may_complete_in_timeline(
    workspace: &Workspace,
    scheme_id: SchemeId,
    item_id: ItemId,
) -> bool {
    workspace_item(workspace, scheme_id, item_id).is_some_and(item_may_complete_in_timeline)
}

fn item_may_schedule_notifications(item: &Item) -> bool {
    item.kind() != ItemKind::Procedure
}

fn item_may_complete_in_timeline(item: &Item) -> bool {
    item.kind() == ItemKind::Event
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use knotq_commands::DateKind;
    use knotq_model::Scheme;

    #[test]
    fn text_edits_do_not_wake_background_calendar_workers_for_plain_items() {
        let (workspace, scheme, item) = workspace_with_item(Item::new("plain"));
        let signals = service_signals_for_command(
            &Command::UpdateItemText {
                scheme,
                item,
                text: "typing".to_string(),
            },
            &workspace,
        );

        assert!(matches!(
            signals.notifications,
            NotificationServiceSignal::None
        ));
        assert!(!signals.timeline);
    }

    #[test]
    fn text_edits_wake_notification_worker_for_calendar_items() {
        let (workspace, scheme, item) =
            workspace_with_item(Item::new("event").with_start(Utc::now()));
        let signals = service_signals_for_command(
            &Command::UpdateItemText {
                scheme,
                item,
                text: "typing".to_string(),
            },
            &workspace,
        );

        assert!(matches!(
            signals.notifications,
            NotificationServiceSignal::RefreshItems(items)
                if items == vec![(scheme, item)]
        ));
        assert!(!signals.timeline);
    }

    #[test]
    fn date_edits_wake_timeline_worker() {
        let workspace = Workspace::empty();
        let signals = service_signals_for_command(
            &Command::SetItemDate {
                scheme: SchemeId::new(),
                item: ItemId::new(),
                kind: DateKind::Start,
                date: None,
            },
            &workspace,
        );

        assert!(matches!(
            signals.notifications,
            NotificationServiceSignal::Recompute
        ));
        assert!(signals.timeline);
    }

    fn workspace_with_item(mut item: Item) -> (Workspace, SchemeId, ItemId) {
        let mut workspace = Workspace::empty();
        let scheme_id = SchemeId::new();
        let item_id = ItemId::new();
        item.id = item_id;

        let mut scheme = Scheme::new("Test", 0);
        scheme.id = scheme_id;
        scheme.items.push(item);
        workspace.schemes.insert(scheme_id, scheme);

        (workspace, scheme_id, item_id)
    }
}
