use knotq_commands::Command;
use knotq_model::{Item, ItemId, ItemKind, OccurrenceId, SchemeId, Workspace};

#[cfg(test)]
use super::{UndoNavigationEntry, UndoNavigationSnapshot};

mod apply;
mod navigation;
mod notifications;
mod undo;

#[cfg(test)]
fn discard_pending_creation_undo_from_stacks(
    undo_stack: &mut std::collections::VecDeque<Command>,
    undo_navigation_stack: &mut std::collections::VecDeque<UndoNavigationEntry>,
    item_id: ItemId,
) -> bool {
    if !pending_creation_undo_matches(undo_stack.back(), item_id) {
        return false;
    }

    undo_stack.pop_back();
    undo_navigation_stack.pop_back();
    true
}

fn pending_creation_undo_matches(cmd: Option<&Command>, item_id: ItemId) -> bool {
    cmd.is_some_and(|cmd| matches!(cmd, Command::DeleteItem { item, .. } if *item == item_id))
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

fn collect_completion_candidates(cmd: &Command, out: &mut Vec<(SchemeId, ItemId, OccurrenceId)>) {
    match cmd {
        Command::ToggleOccurrence {
            scheme,
            item,
            occurrence,
        } => out.push((*scheme, *item, occurrence.clone())),
        Command::ReplaceItem { scheme, item } | Command::InsertItem { scheme, item, .. } => {
            collect_done_occurrences(*scheme, item, out);
        }
        Command::Batch(cmds) => {
            for cmd in cmds {
                collect_completion_candidates(cmd, out);
            }
        }
        _ => {}
    }
}

fn collect_done_occurrences(
    scheme: SchemeId,
    item: &Item,
    out: &mut Vec<(SchemeId, ItemId, OccurrenceId)>,
) {
    for state in &item.state {
        if state.state.is_done() {
            out.push((scheme, item.id, state.occurrence.clone()));
        }
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
        | Command::RestoreDeletedFolder { .. }
        | Command::PermanentlyDeleteFolder { .. }
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
    use std::collections::VecDeque;

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

    #[test]
    fn discarding_pending_creation_removes_top_delete_undo() {
        let scheme = SchemeId::new();
        let item = ItemId::new();
        let older_item = ItemId::new();
        let mut undo_stack = VecDeque::from([
            Command::DeleteItem {
                scheme,
                item: older_item,
            },
            Command::DeleteItem { scheme, item },
        ]);
        let mut navigation_stack = VecDeque::from([undo_nav(), undo_nav()]);

        assert!(discard_pending_creation_undo_from_stacks(
            &mut undo_stack,
            &mut navigation_stack,
            item,
        ));

        assert_eq!(undo_stack.len(), 1);
        assert_eq!(navigation_stack.len(), 1);
        assert!(matches!(
            undo_stack.back(),
            Some(Command::DeleteItem { item, .. }) if *item == older_item
        ));
    }

    #[test]
    fn discarding_pending_creation_ignores_non_top_delete_undo() {
        let scheme = SchemeId::new();
        let item = ItemId::new();
        let newer_item = ItemId::new();
        let mut undo_stack = VecDeque::from([
            Command::DeleteItem { scheme, item },
            Command::DeleteItem {
                scheme,
                item: newer_item,
            },
        ]);
        let mut navigation_stack = VecDeque::from([undo_nav(), undo_nav()]);

        assert!(!discard_pending_creation_undo_from_stacks(
            &mut undo_stack,
            &mut navigation_stack,
            item,
        ));

        assert_eq!(undo_stack.len(), 2);
        assert_eq!(navigation_stack.len(), 2);
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

    fn undo_nav() -> UndoNavigationEntry {
        let snapshot = UndoNavigationSnapshot {
            selection: Default::default(),
            week_offset: 0,
            month_offset: 0,
        };
        UndoNavigationEntry {
            before: snapshot.clone(),
            after: snapshot,
        }
    }
}
