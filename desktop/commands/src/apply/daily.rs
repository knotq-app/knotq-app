use chrono::NaiveDate;
use knotq_model::{
    daily_queue_placeholder_item_id, daily_queue_scheme_id, daily_queue_scheme_name, Item, Scheme,
    Workspace, DAILY_QUEUE_COLOR_INDEX,
};

use crate::invariants::CommandError;
use crate::{ChangeSet, Command, CommandReceipt};

/// Make `date`'s Daily Queue page exist and be editable: bind and create the
/// day if it has no scheme, and give an empty day its blank placeholder row.
///
/// The one way a day comes into being, on every client. A day that is already
/// bound but whose scheme is not in `workspace` is created here too — so the
/// caller must first load or materialize it (from disk or its CRDT document)
/// if it has any content. Creating an empty page over content this device has
/// not loaded yet is how a day gets wiped.
///
/// Days are not undoable: the inverse is an empty batch.
pub(crate) fn ensure_daily_queue(
    workspace: &mut Workspace,
    date: NaiveDate,
) -> Result<CommandReceipt, CommandError> {
    let id = workspace
        .daily_queue_scheme_id(date)
        .unwrap_or_else(|| daily_queue_scheme_id(date));
    workspace.daily_queue.insert(date, id);
    // The placeholder's id is derived from the date so that two devices
    // creating the same day converge on one blank row instead of two. That
    // same determinism makes it re-creatable after the row has *moved*: a
    // carry-over (or an ordinary drag) takes the row — id and all — to another
    // page, leaving this day empty, and re-minting the id here would put the
    // one item in two schemes at once. Item ids are globally unique, and the
    // CRDT enforces that by hiding the duplicate in whichever scheme loses
    // `dedupe_materialized_items` — so the resurrected row is invisible to
    // every document-derived view while the plain workspace still shows it,
    // and the day silently empties again on the next sync. Leave the day blank
    // instead; the row is alive where the user moved it, and the next typed
    // line creates a new one.
    let placeholder_id = daily_queue_placeholder_item_id(date);
    let placeholder_lives_elsewhere = workspace.schemes.iter().any(|(scheme_id, scheme)| {
        *scheme_id != id && scheme.items.iter().any(|item| item.id == placeholder_id)
    });
    let scheme = workspace.schemes.entry(id).or_insert_with(|| {
        let mut scheme = Scheme::new(daily_queue_scheme_name(date), DAILY_QUEUE_COLOR_INDEX);
        scheme.id = id;
        scheme
    });
    if scheme.items.is_empty() && !placeholder_lives_elsewhere {
        let mut placeholder = Item::new("");
        placeholder.id = placeholder_id;
        scheme.items.push(placeholder);
    }
    Ok(CommandReceipt {
        inverse: Command::Batch(Vec::new()),
        touched: ChangeSet::default().touched_scheme(id),
    })
}
