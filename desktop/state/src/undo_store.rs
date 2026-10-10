use std::collections::{HashMap, HashSet, VecDeque};

use knotq_commands::Command;
use knotq_model::{ItemId, SchemeId, Workspace};

use crate::undo::UNDO_DEPTH;
use crate::Selection;

/// Where the user was (and what was focused) around a command, captured so undo
/// and redo can return the UI to the place the change happened rather than
/// leaving the user wherever they drifted to.
///
/// The `cursor` slot is populated by the live app path (the editor owns the
/// text caret); the pure state-crate path leaves it `None`.
#[derive(Clone, Debug, Default)]
pub struct NavSnapshot {
    pub selection: Selection,
    pub week_offset: i32,
    pub month_offset: i32,
}

/// One undoable step: the inverse command to apply, the timeline it belongs to,
/// and the navigation snapshots bracketing the original command. `before` is
/// restored on undo, `after` on redo. Fusing command + scope + navigation into a
/// single entry keeps them from drifting out of lockstep (the previous design
/// kept them in parallel stacks aligned only by index position). `scope` is
/// stamped from where the action was *initiated*, not just what it touches — see
/// [`UndoScope::for_command`].
#[derive(Clone, Debug)]
pub struct UndoEntry {
    pub inverse: Command,
    pub scope: UndoScope,
    pub before: NavSnapshot,
    pub after: NavSnapshot,
}

/// Which timeline an undoable step belongs to.
///
/// - `Workspace`: structural operations on the workspace itself (create/rename/
///   move/delete a scheme or folder). Undone from views with no focused scheme.
/// - `Scheme(id)`: a content edit confined to one scheme, made *while that scheme
///   was focused*. Undone while that scheme is focused.
/// - `Global`: a step undone from the global (calendar) view — either it edits
///   items across two or more schemes (e.g. carrying an item between daily
///   queues), or it was initiated from the calendar where there is no focused
///   scheme. Like an IDE's global action, it can touch per-scheme content yet is
///   undone globally.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UndoScope {
    Workspace,
    Scheme(SchemeId),
    Global,
}

/// Which schemes a command's *content* touches, and whether it performs any
/// workspace-structural operation. Computed by an exhaustive match so adding a
/// new `Command` variant forces an explicit classification decision here.
struct Touched {
    structural: bool,
    schemes: Vec<SchemeId>,
}

fn touched(cmd: &Command) -> Touched {
    match cmd {
        Command::InsertItem { scheme, .. }
        | Command::UpdateItemText { scheme, .. }
        | Command::ReplaceItem { scheme, .. }
        | Command::SetItemIndent { scheme, .. }
        | Command::SetItemMarker { scheme, .. }
        | Command::SetItemMarkerFamily { scheme, .. }
        | Command::SetItemDate { scheme, .. }
        | Command::SetItemRecurrence { scheme, .. }
        | Command::SetItemPriority { scheme, .. }
        | Command::SetOccurrenceNotificationOffset { scheme, .. }
        | Command::ToggleOccurrence { scheme, .. }
        | Command::DeleteItem { scheme, .. }
        | Command::ReorderItem { scheme, .. } => Touched {
            structural: false,
            schemes: vec![*scheme],
        },
        Command::CreateFolder { .. }
        | Command::RestoreFolder { .. }
        | Command::RestoreDeletedFolder { .. }
        | Command::RenameFolder { .. }
        | Command::SetFolderExpanded { .. }
        | Command::DeleteFolder { .. }
        | Command::PermanentlyDeleteFolder { .. }
        | Command::CreateScheme { .. }
        | Command::RestoreScheme { .. }
        | Command::RestoreDeletedScheme { .. }
        | Command::RenameScheme { .. }
        | Command::SetSchemeColor { .. }
        | Command::SetSchemeGsync { .. }
        | Command::SetSchemeSource { .. }
        | Command::DeleteScheme { .. }
        | Command::PermanentlyDeleteScheme { .. }
        | Command::MoveNode { .. } => Touched {
            structural: true,
            schemes: Vec::new(),
        },
        Command::EnsureDailyQueue { date } => Touched {
            structural: true,
            schemes: vec![knotq_model::daily_queue_scheme_id(*date)],
        },
        Command::Batch(cmds) => {
            let mut acc = Touched {
                structural: false,
                schemes: Vec::new(),
            };
            for cmd in cmds {
                let t = touched(cmd);
                acc.structural |= t.structural;
                for scheme in t.schemes {
                    if !acc.schemes.contains(&scheme) {
                        acc.schemes.push(scheme);
                    }
                }
            }
            acc
        }
    }
}

impl UndoScope {
    /// Decide the timeline a freshly applied command files under, combining the
    /// command's structure with where it was initiated (`active`, the scope of
    /// the view in focus when it ran):
    ///
    /// - any structural operation → `Workspace` (even a scheme+content batch, so
    ///   it undoes from the workspace, never from inside the spawned scheme);
    /// - a cross-scheme content edit (≥2 schemes) → `Global`;
    /// - a single-scheme content edit made while that scheme is focused →
    ///   `Scheme`; the same edit initiated from the calendar (or any other view)
    ///   → `Global`, so a calendar action undoes from the calendar.
    pub fn for_command(cmd: &Command, active: UndoScope) -> UndoScope {
        let t = touched(cmd);
        if t.structural {
            return UndoScope::Workspace;
        }
        match t.schemes.len() {
            0 => UndoScope::Workspace,
            1 => {
                let scheme = t.schemes[0];
                if active == UndoScope::Scheme(scheme) {
                    UndoScope::Scheme(scheme)
                } else {
                    UndoScope::Global
                }
            }
            _ => UndoScope::Global,
        }
    }
}

/// Does an entry's stamped scope satisfy the currently active scope (i.e. would
/// the user expect this entry when they press undo right now)? A focused scheme
/// sees only its own edits; the global (calendar) view sees workspace-structural
/// steps and global steps — never another scheme's private edits.
fn entry_matches(scope: UndoScope, active: UndoScope) -> bool {
    match active {
        UndoScope::Scheme(a) => scope == UndoScope::Scheme(a),
        UndoScope::Workspace => matches!(scope, UndoScope::Workspace | UndoScope::Global),
        UndoScope::Global => false,
    }
}

/// Bounded undo/redo history. Entries live in one time-ordered list; undo and
/// redo select the most recent entry matching the active scope. This yields
/// strict per-scheme LIFO (the most recent entry touching a scheme is always
/// that scheme's next undo, with other schemes' entries simply skipped) while
/// keeping cross-scheme steps reachable from either side.
#[derive(Clone, Debug)]
pub struct UndoStore {
    undo: VecDeque<UndoEntry>,
    redo: VecDeque<UndoEntry>,
    max_depth: usize,
}

impl Default for UndoStore {
    fn default() -> Self {
        Self::new(UNDO_DEPTH)
    }
}

impl UndoStore {
    pub fn new(max_depth: usize) -> Self {
        Self {
            undo: VecDeque::new(),
            redo: VecDeque::new(),
            max_depth,
        }
    }

    /// Record a freshly applied command's inverse. Drops the oldest entry when
    /// the depth cap is exceeded.
    pub fn push_undo(&mut self, entry: UndoEntry) {
        self.undo.push_back(entry);
        while self.undo.len() > self.max_depth {
            self.undo.pop_front();
        }
    }

    /// Remove and return the most recent undo entry matching `active` (its
    /// inverse is what the caller applies). Entries belonging to other scopes
    /// are left in place.
    pub fn take_undo(&mut self, active: UndoScope) -> Option<UndoEntry> {
        let idx = self
            .undo
            .iter()
            .rposition(|entry| entry_matches(entry.scope, active))?;
        self.undo.remove(idx)
    }

    /// Remove and return the most recent redo entry matching `active`.
    pub fn take_redo(&mut self, active: UndoScope) -> Option<UndoEntry> {
        let idx = self
            .redo
            .iter()
            .rposition(|entry| entry_matches(entry.scope, active))?;
        self.redo.remove(idx)
    }

    /// Push an entry onto the redo side (used after an undo applies cleanly).
    pub fn record_redo(&mut self, entry: UndoEntry) {
        self.redo.push_back(entry);
    }

    /// Drop redo entries that a freshly applied command invalidates: anything
    /// sharing a touched scheme, plus all workspace-scoped redo when the new
    /// command is itself workspace-scoped. Redo on untouched scopes survives,
    /// so a fresh edit in scheme A doesn't discard a pending redo in scheme B.
    pub fn clear_redo_conflicting(&mut self, cmd: &Command) {
        let t = touched(cmd);
        let cmd_workspace = t.structural || t.schemes.is_empty();
        self.redo.retain(|entry| {
            let et = touched(&entry.inverse);
            let entry_workspace = et.structural || et.schemes.is_empty();
            let conflict = (cmd_workspace && entry_workspace)
                || t.schemes.iter().any(|s| et.schemes.contains(s));
            !conflict
        });
    }

    /// Drop the entire history (e.g. when the workspace is replaced wholesale).
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }

    /// Peek at the most recently recorded entry (used to retarget/discard a
    /// still-pending creation, which is always the latest push).
    pub fn last_undo(&self) -> Option<&UndoEntry> {
        self.undo.back()
    }

    /// Mutable peek — used to retarget a still-pending creation's inverse (e.g.
    /// when an event drafted on the calendar is committed into a real scheme).
    pub fn last_undo_mut(&mut self) -> Option<&mut UndoEntry> {
        self.undo.back_mut()
    }

    /// Discard the most recent undo entry without applying it (used to drop a
    /// provisional creation's undo when the creation is cancelled/committed).
    pub fn discard_last_undo(&mut self) -> Option<UndoEntry> {
        self.undo.pop_back()
    }

    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    pub fn redo_len(&self) -> usize {
        self.redo.len()
    }

    /// Discard the undo/redo entries a sync landing has invalidated, and no
    /// others. An entry is dropped only if replaying its inverse could now do
    /// something other than what it recorded: it addresses a line the sync
    /// changed, or it is positional and the sync moved lines around. Edits to
    /// the *other* lines of a scheme a remote device has just touched stay
    /// undoable; workspace-structural entries always survive.
    pub fn clear_invalidated_by_sync(&mut self, delta: &SyncDelta) {
        let keep = |entry: &UndoEntry| match entry.scope {
            UndoScope::Workspace => true,
            UndoScope::Scheme(_) | UndoScope::Global => !delta.invalidates(&entry.inverse),
        };
        self.undo.retain(keep);
        self.redo.retain(keep);
    }
}

/// What a sync landing changed, at the granularity undo needs: per scheme,
/// which lines differ and whether any line was added, removed or moved.
#[derive(Clone, Debug, Default)]
pub struct SyncDelta {
    schemes: HashMap<SchemeId, SchemeDelta>,
}

#[derive(Clone, Debug)]
enum SchemeDelta {
    /// The scheme appeared or disappeared; nothing recorded against it holds.
    Whole,
    Lines {
        /// Lines whose content differs, plus any present on only one side.
        changed: HashSet<ItemId>,
        /// The sequence of line ids differs, so an index no longer means the
        /// line it meant.
        order_changed: bool,
    },
}

impl SyncDelta {
    /// The difference between the workspace before a landing and after it.
    /// Schemes whose lines are equal are absent.
    pub fn between(before: &Workspace, after: &Workspace) -> Self {
        let mut schemes = HashMap::new();
        for (scheme_id, old) in &before.schemes {
            let Some(new) = after.schemes.get(scheme_id) else {
                schemes.insert(*scheme_id, SchemeDelta::Whole);
                continue;
            };
            if new.items == old.items {
                continue;
            }
            let old_by_id: HashMap<ItemId, &knotq_model::Item> =
                old.items.iter().map(|item| (item.id, item)).collect();
            let mut changed: HashSet<ItemId> = new
                .items
                .iter()
                .filter(|item| old_by_id.get(&item.id) != Some(item))
                .map(|item| item.id)
                .collect();
            let new_ids: HashSet<ItemId> = new.items.iter().map(|item| item.id).collect();
            changed.extend(old_by_id.keys().filter(|id| !new_ids.contains(id)));
            let order_changed = old.items.len() != new.items.len()
                || old
                    .items
                    .iter()
                    .zip(new.items.iter())
                    .any(|(old, new)| old.id != new.id);
            schemes.insert(
                *scheme_id,
                SchemeDelta::Lines {
                    changed,
                    order_changed,
                },
            );
        }
        for scheme_id in after.schemes.keys() {
            if !before.schemes.contains_key(scheme_id) {
                schemes.insert(*scheme_id, SchemeDelta::Whole);
            }
        }
        Self { schemes }
    }

    /// Whether the landing changed this line (or its whole scheme).
    pub fn changed_line(&self, scheme: SchemeId, item: ItemId) -> bool {
        match self.schemes.get(&scheme) {
            None => false,
            Some(SchemeDelta::Whole) => true,
            Some(SchemeDelta::Lines { changed, .. }) => changed.contains(&item),
        }
    }

    fn reordered(&self, scheme: SchemeId) -> bool {
        match self.schemes.get(&scheme) {
            None => false,
            Some(SchemeDelta::Whole) => true,
            Some(SchemeDelta::Lines { order_changed, .. }) => *order_changed,
        }
    }

    /// Whether replaying `cmd` after this landing could do something other
    /// than what it was recorded to do. Exhaustive, so a new `Command` variant
    /// has to say how it addresses what it changes.
    fn invalidates(&self, cmd: &Command) -> bool {
        match cmd {
            // Addressed by line id: still exactly right unless that line changed.
            Command::UpdateItemText { scheme, item, .. }
            | Command::SetItemIndent { scheme, item, .. }
            | Command::SetItemMarker { scheme, item, .. }
            | Command::SetItemMarkerFamily { scheme, item, .. }
            | Command::SetItemDate { scheme, item, .. }
            | Command::SetItemRecurrence { scheme, item, .. }
            | Command::SetItemPriority { scheme, item, .. }
            | Command::SetOccurrenceNotificationOffset { scheme, item, .. }
            | Command::ToggleOccurrence { scheme, item, .. }
            | Command::DeleteItem { scheme, item } => self.changed_line(*scheme, *item),
            Command::ReplaceItem { scheme, item } => self.changed_line(*scheme, item.id),
            // Addressed by position: an index only means the same line while
            // the scheme's lines are in the same order.
            Command::InsertItem { scheme, item, .. } => {
                self.reordered(*scheme) || self.changed_line(*scheme, item.id)
            }
            Command::ReorderItem { scheme, .. } => self.reordered(*scheme),
            // Workspace structure is not what a landing's line delta describes;
            // these apply or fail on their own terms, as they always have.
            Command::CreateFolder { .. }
            | Command::RestoreFolder { .. }
            | Command::RestoreDeletedFolder { .. }
            | Command::RenameFolder { .. }
            | Command::SetFolderExpanded { .. }
            | Command::DeleteFolder { .. }
            | Command::PermanentlyDeleteFolder { .. }
            | Command::CreateScheme { .. }
            | Command::RestoreScheme { .. }
            | Command::RestoreDeletedScheme { .. }
            | Command::RenameScheme { .. }
            | Command::SetSchemeColor { .. }
            | Command::SetSchemeGsync { .. }
            | Command::SetSchemeSource { .. }
            | Command::DeleteScheme { .. }
            | Command::PermanentlyDeleteScheme { .. }
            | Command::MoveNode { .. }
            | Command::EnsureDailyQueue { .. } => false,
            Command::Batch(commands) => commands.iter().any(|cmd| self.invalidates(cmd)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use knotq_model::ItemId;

    fn entry(inverse: Command, scope: UndoScope) -> UndoEntry {
        UndoEntry {
            inverse,
            scope,
            before: NavSnapshot::default(),
            after: NavSnapshot::default(),
        }
    }

    fn delete(scheme: SchemeId) -> Command {
        Command::DeleteItem {
            scheme,
            item: ItemId::new(),
        }
    }

    fn cross(a: SchemeId, b: SchemeId) -> Command {
        Command::Batch(vec![delete(a), delete(b)])
    }

    #[test]
    fn for_command_classifies_by_origin() {
        let a = SchemeId::new();
        let b = SchemeId::new();
        // Same single-scheme edit: scheme-local when made in its scheme, global
        // when initiated from the calendar or any other view.
        assert_eq!(
            UndoScope::for_command(&delete(a), UndoScope::Scheme(a)),
            UndoScope::Scheme(a)
        );
        assert_eq!(
            UndoScope::for_command(&delete(a), UndoScope::Workspace),
            UndoScope::Global
        );
        assert_eq!(
            UndoScope::for_command(&delete(a), UndoScope::Scheme(b)),
            UndoScope::Global
        );
        // Structural is always workspace; cross-scheme is always global.
        assert_eq!(
            UndoScope::for_command(&Command::DeleteScheme { id: a }, UndoScope::Scheme(a)),
            UndoScope::Workspace
        );
        assert_eq!(
            UndoScope::for_command(&cross(a, b), UndoScope::Scheme(a)),
            UndoScope::Global
        );
        let mixed = Command::Batch(vec![Command::DeleteScheme { id: a }, delete(b)]);
        assert_eq!(
            UndoScope::for_command(&mixed, UndoScope::Scheme(b)),
            UndoScope::Workspace
        );
    }

    #[test]
    fn scheme_take_skips_other_schemes() {
        let a = SchemeId::new();
        let b = SchemeId::new();
        let mut store = UndoStore::default();
        store.push_undo(entry(delete(a), UndoScope::Scheme(a)));
        store.push_undo(entry(delete(b), UndoScope::Scheme(b)));
        let taken = store.take_undo(UndoScope::Scheme(a)).unwrap();
        assert!(matches!(&taken.inverse, Command::DeleteItem { scheme, .. } if *scheme == a));
        assert_eq!(store.undo_len(), 1); // the newer B entry is left in place
    }

    #[test]
    fn workspace_take_sees_workspace_and_global_not_scheme() {
        let a = SchemeId::new();
        let mut store = UndoStore::default();
        store.push_undo(entry(delete(a), UndoScope::Scheme(a)));
        store.push_undo(entry(Command::DeleteScheme { id: a }, UndoScope::Workspace));
        store.push_undo(entry(delete(a), UndoScope::Global));

        // The global view takes the most recent global/workspace step (global).
        assert_eq!(
            store.take_undo(UndoScope::Workspace).unwrap().scope,
            UndoScope::Global
        );
        // The scheme's own private edit is reachable only from the scheme.
        assert_eq!(
            store.take_undo(UndoScope::Scheme(a)).unwrap().scope,
            UndoScope::Scheme(a)
        );
        // ...then the workspace structural step is what remains for the global view.
        assert_eq!(
            store.take_undo(UndoScope::Workspace).unwrap().scope,
            UndoScope::Workspace
        );
    }

    #[test]
    fn global_step_is_undone_from_the_global_view_only() {
        let a = SchemeId::new();
        let b = SchemeId::new();

        let mut store = UndoStore::default();
        store.push_undo(entry(cross(a, b), UndoScope::Global));
        // Not reachable from any scheme...
        assert!(store.take_undo(UndoScope::Scheme(a)).is_none());
        assert!(store.take_undo(UndoScope::Scheme(b)).is_none());
        // ...only from the global (calendar) view.
        assert!(store.take_undo(UndoScope::Workspace).is_some());
    }

    #[test]
    fn redo_clear_spares_untouched_schemes() {
        let a = SchemeId::new();
        let b = SchemeId::new();
        let mut store = UndoStore::default();
        store.record_redo(entry(delete(a), UndoScope::Scheme(a)));
        store.record_redo(entry(delete(b), UndoScope::Scheme(b)));
        store.clear_redo_conflicting(&delete(a));
        assert!(store.take_redo(UndoScope::Scheme(a)).is_none());
        assert!(store.take_redo(UndoScope::Scheme(b)).is_some());
    }
}
