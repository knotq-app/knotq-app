use knotq_commands::{Command, WorkspaceCommandExt};
use knotq_model::{Item, ItemId, Scheme, Workspace};

/// A batch must be all-or-nothing: when a later leg fails, the legs already
/// applied are rolled back so the workspace is left exactly as it was. This is
/// what lets a cross-scheme undo whose second leg no longer applies fail
/// cleanly instead of corrupting the workspace.
#[test]
fn failed_batch_rolls_back_earlier_legs() {
    let mut workspace = Workspace::new();
    let mut scheme = Scheme::new("S", 0);
    let scheme_id = scheme.id;
    let item = Item::new("original");
    let item_id = item.id;
    scheme.items.push(item);
    workspace.schemes.insert(scheme_id, scheme);

    let missing = ItemId::new();
    let result = workspace.apply(Command::Batch(vec![
        Command::UpdateItemText {
            scheme: scheme_id,
            item: item_id,
            text: "changed".into(),
        },
        Command::UpdateItemText {
            scheme: scheme_id,
            item: missing,
            text: "boom".into(),
        },
    ]));

    assert!(result.is_err());
    assert_eq!(
        workspace
            .scheme(scheme_id)
            .and_then(|scheme| scheme.item(item_id))
            .map(|item| item.text())
            .as_deref(),
        Some("original"),
        "the first leg should have been rolled back"
    );
}
