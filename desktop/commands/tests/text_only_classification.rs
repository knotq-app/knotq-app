//! `Command::changes_only_item_text` lets views keep a schedule they derived
//! from the workspace across such a command. That is only sound if the command
//! really does leave everything but item body text alone — so rather than
//! trusting the classification, apply the command and check.

use knotq_commands::{Command, CommandOrigin, WorkspaceCommandExt};
use knotq_model::{
    CalendarRecurrence, Item, ItemMarker, NodeRef, Scheme, SchemeId, Workspace,
};

fn workspace_with_rich_items() -> (Workspace, SchemeId) {
    let mut workspace = Workspace::new();
    let mut scheme = Scheme::new("scheme", 2);

    let mut plain = Item::new("plain");
    plain.marker = ItemMarker::Checkbox;

    let mut dated = Item::new("dated");
    dated.marker = ItemMarker::Checkbox;
    dated.start = Some(chrono::Utc::now());
    dated.end = Some(chrono::Utc::now() + chrono::Duration::hours(1));
    dated.priority = Some(1);
    dated.indent = 2;

    let mut repeating = Item::new("repeating");
    repeating.marker = ItemMarker::Checkbox;
    repeating.start = Some(chrono::Utc::now());
    repeating.repeats = Some(CalendarRecurrence {
        rrules: vec!["FREQ=WEEKLY;INTERVAL=1".to_string()],
        ..Default::default()
    });

    scheme.items = vec![plain, dated, repeating];
    let scheme_id = scheme.id;
    workspace
        .folders
        .get_mut(&workspace.root)
        .unwrap()
        .children
        .push(NodeRef::Scheme(scheme_id));
    workspace.schemes.insert(scheme_id, scheme);
    workspace.ensure_sync_metadata();
    (workspace, scheme_id)
}

/// Everything about an item except its body text, so a text edit can be shown to
/// leave all of it untouched.
fn schedule_fingerprint(workspace: &Workspace) -> Vec<String> {
    let mut out = Vec::new();
    let mut scheme_ids: Vec<_> = workspace.schemes.keys().copied().collect();
    scheme_ids.sort_by_key(|id| id.to_string());
    for scheme_id in scheme_ids {
        let scheme = &workspace.schemes[&scheme_id];
        out.push(format!(
            "scheme {scheme_id} name={} color={} source={:?}",
            scheme.name, scheme.color_index, scheme.source
        ));
        for (position, item) in scheme.items.iter().enumerate() {
            out.push(format!(
                "  [{position}] id={} marker={:?} start={:?} end={:?} available={:?} \
                 repeats={:?} priority={:?} indent={} state={:?} external={:?}",
                item.id,
                item.marker,
                item.start,
                item.end,
                item.available,
                item.repeats,
                item.priority,
                item.indent,
                item.state,
                item.external,
            ));
        }
    }
    let mut folder_ids: Vec<_> = workspace.folders.keys().copied().collect();
    folder_ids.sort_by_key(|id| id.to_string());
    for folder_id in folder_ids {
        let folder = &workspace.folders[&folder_id];
        out.push(format!(
            "folder {folder_id} name={} children={:?}",
            folder.name, folder.children
        ));
    }
    out.push(format!("daily_queue={:?}", workspace.daily_queue));
    out.push(format!("recently_deleted={:?}", workspace.recently_deleted));
    out
}

#[test]
fn update_item_text_changes_nothing_but_the_text() {
    let (mut workspace, scheme) = workspace_with_rich_items();
    let before = schedule_fingerprint(&workspace);

    for index in 0..3 {
        let item = workspace.schemes[&scheme].items[index].id;
        let command = Command::UpdateItemText {
            scheme,
            item,
            text: format!("rewritten {index}"),
        };
        assert!(command.changes_only_item_text());
        workspace.apply(command).unwrap();
    }

    assert_eq!(
        schedule_fingerprint(&workspace),
        before,
        "UpdateItemText altered something other than item body text, so the \
         text-only classification is unsound"
    );
    for index in 0..3 {
        assert_eq!(
            workspace.schemes[&scheme].items[index].text(),
            format!("rewritten {index}")
        );
    }
}

/// The inverse command a text edit produces is itself a text edit, so undoing a
/// typing burst must not be misclassified as a schedule change either — and, more
/// importantly, must not be misclassified as text-only when it is not.
#[test]
fn the_inverse_of_a_text_edit_is_also_text_only() {
    let (mut workspace, scheme) = workspace_with_rich_items();
    let item = workspace.schemes[&scheme].items[0].id;
    let receipt = workspace
        .apply(Command::UpdateItemText {
            scheme,
            item,
            text: "typed".into(),
        })
        .unwrap();
    assert!(
        receipt.inverse.changes_only_item_text(),
        "undoing a text edit is still a text-only change"
    );

    let before = schedule_fingerprint(&workspace);
    workspace.apply(receipt.inverse).unwrap();
    assert_eq!(schedule_fingerprint(&workspace), before);
    assert_eq!(workspace.schemes[&scheme].items[0].text(), "plain");
}

/// Non-text commands must never be classified as text-only. Checked directly on
/// the classifier so a newly added variant that someone wires in as text-only
/// has to justify itself here.
#[test]
fn schedule_changing_commands_are_never_text_only() {
    let (workspace, scheme) = workspace_with_rich_items();
    let item = workspace.schemes[&scheme].items[0].id;
    let root = workspace.root;

    let commands = vec![
        Command::SetItemMarker {
            scheme,
            item,
            marker: ItemMarker::Bullet,
        },
        Command::SetItemDate {
            scheme,
            item,
            kind: knotq_commands::DateKind::Start,
            date: None,
        },
        Command::SetItemRecurrence {
            scheme,
            item,
            repeats: None,
        },
        Command::SetItemPriority {
            scheme,
            item,
            priority: None,
        },
        Command::SetItemIndent {
            scheme,
            item,
            indent: 0,
        },
        Command::ToggleOccurrence {
            scheme,
            item,
            occurrence: knotq_model::OccurrenceId::Single,
        },
        Command::SetOccurrenceNotificationOffset {
            scheme,
            item,
            occurrence: knotq_model::OccurrenceId::Single,
            offset_secs: Some(60),
        },
        Command::ReplaceItem {
            scheme,
            item: workspace.schemes[&scheme].items[0].clone(),
        },
        Command::InsertItem {
            scheme,
            position: 0,
            item: Item::new("x"),
        },
        Command::DeleteItem { scheme, item },
        Command::ReorderItem {
            scheme,
            from: 0,
            to: 1,
        },
        Command::RenameScheme {
            id: scheme,
            name: "x".into(),
        },
        Command::SetSchemeColor {
            id: scheme,
            color_index: 1,
        },
        Command::SetSchemeGsync {
            id: scheme,
            on: true,
        },
        Command::DeleteScheme { id: scheme },
        Command::PermanentlyDeleteScheme { id: scheme },
        Command::CreateScheme {
            folder: root,
            name: "x".into(),
            color_index: 0,
            position: None,
        },
        Command::CreateFolder {
            parent: root,
            name: "x".into(),
            position: None,
        },
        Command::RenameFolder {
            id: root,
            name: "x".into(),
        },
        Command::SetFolderExpanded {
            id: root,
            expanded: false,
        },
        Command::DeleteFolder { id: root },
        Command::PermanentlyDeleteFolder { id: root },
        Command::MoveNode {
            node: NodeRef::Scheme(scheme),
            new_parent: root,
            position: 0,
        },
    ];

    for command in commands {
        assert!(
            !command.changes_only_item_text(),
            "{command:?} must not be classified as text-only"
        );
        // A batch is text-only only if every member is.
        assert!(!Command::Batch(vec![
            Command::UpdateItemText {
                scheme,
                item,
                text: "t".into()
            },
            command.clone(),
        ])
        .changes_only_item_text());
    }
}

/// Nesting must not let a schedule change hide inside a batch.
#[test]
fn nested_batches_are_classified_by_their_contents() {
    let (workspace, scheme) = workspace_with_rich_items();
    let item = workspace.schemes[&scheme].items[0].id;
    let text = || Command::UpdateItemText {
        scheme,
        item,
        text: "t".into(),
    };

    assert!(Command::Batch(vec![text(), text()]).changes_only_item_text());
    assert!(Command::Batch(vec![Command::Batch(vec![text()]), text()]).changes_only_item_text());
    assert!(!Command::Batch(vec![
        Command::Batch(vec![
            text(),
            Command::DeleteItem { scheme, item },
        ]),
        text(),
    ])
    .changes_only_item_text());
    // An empty batch changes nothing, but claiming "text-only" for it would let
    // an empty nested batch make a non-empty parent look text-only.
    assert!(!Command::Batch(vec![]).changes_only_item_text());
    assert!(!Command::Batch(vec![Command::Batch(vec![])]).changes_only_item_text());
}

/// End-to-end through the command dispatcher, since that is how the app applies
/// them.
#[test]
fn applying_a_text_only_batch_leaves_the_schedule_alone() {
    let (mut workspace, scheme) = workspace_with_rich_items();
    let before = schedule_fingerprint(&workspace);
    let items: Vec<_> = workspace.schemes[&scheme]
        .items
        .iter()
        .map(|item| item.id)
        .collect();
    let batch = Command::Batch(
        items
            .iter()
            .map(|item| Command::UpdateItemText {
                scheme,
                item: *item,
                text: format!("batched {item}"),
            })
            .collect(),
    );
    assert!(batch.changes_only_item_text());
    workspace.apply(batch).unwrap();
    assert_eq!(schedule_fingerprint(&workspace), before);
    let _ = CommandOrigin::User;
}
