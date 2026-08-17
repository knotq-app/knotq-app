//! Randomized equivalence check for the sync-metadata fast path.
//!
//! `ensure_sync_metadata` repairs the scheme/folder→document bindings and runs on
//! every applied command. It is now gated on `sync_metadata_is_current`, a
//! read-only predicate that must be *exactly* the negation of "the repair pass
//! would change something". If the predicate ever said "current" about a
//! workspace that actually needed repair, the binding would silently stay broken
//! — the failure mode that orphaned real content in the past (see the
//! `scheme_binding_remint_is_deterministic_and_convergent` regression).
//!
//! So rather than only checking hand-picked cases, this mutilates workspaces at
//! random and asserts the two agree on every one.

use std::collections::HashMap;

use chrono::NaiveDate;
use knotq_model::{
    daily_queue_document_id, daily_queue_scheme_id, CrdtBackend, DocumentId, FolderId, Item,
    NodeRef, Scheme, SchemeId, SyncDocumentKind, SyncDocumentMeta, Workspace,
};

/// Deterministic PRNG (SplitMix64) so a failure reproduces from its seed.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }

    fn chance(&mut self, one_in: u64) -> bool {
        self.next_u64() % one_in == 0
    }
}

fn date_for(n: u64) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 1 + (n % 12) as u32, 1 + (n % 28) as u32).unwrap()
}

/// A workspace with a mix of ordinary schemes, folders, and daily-queue days.
fn seeded_workspace(rng: &mut Rng) -> Workspace {
    let mut workspace = Workspace::new();
    let root = workspace.root;

    for i in 0..rng.below(6) {
        let mut folder = knotq_model::Folder {
            id: FolderId::new(),
            name: format!("folder-{i}"),
            parent: Some(root),
            children: Vec::new(),
            expanded: true,
        };
        folder.children.clear();
        workspace.folders.insert(folder.id, folder);
    }

    for i in 0..rng.below(8) {
        let mut scheme = Scheme::new(format!("scheme-{i}"), (i % 8) as u8);
        scheme.items = vec![Item::new("a"), Item::new("b")];
        workspace
            .folders
            .get_mut(&root)
            .unwrap()
            .children
            .push(NodeRef::Scheme(scheme.id));
        workspace.schemes.insert(scheme.id, scheme);
    }

    for i in 0..rng.below(5) {
        let date = date_for(i as u64 + rng.next_u64());
        // Half the days use the derived (stable) id, half a legacy random one —
        // the two take different branches in both the predicate and the repair.
        let id = if rng.chance(2) {
            daily_queue_scheme_id(date)
        } else {
            SchemeId::new()
        };
        let mut scheme = Scheme::new("Daily", 0);
        scheme.id = id;
        workspace.schemes.insert(id, scheme);
        workspace.daily_queue.insert(date, id);
    }

    workspace
}

/// Break the workspace's sync bindings in a random way.
fn corrupt(workspace: &mut Workspace, rng: &mut Rng) {
    let scheme_ids: Vec<SchemeId> = workspace.schemes.keys().copied().collect();
    let folder_ids: Vec<FolderId> = workspace.folders.keys().copied().collect();

    match rng.below(10) {
        0 => workspace.sync.kind = SyncDocumentKind::Scheme,
        1 => workspace.sync.crdt = CrdtBackend::OperationLog,
        2 => {
            // A binding for a scheme that does not exist.
            workspace.scheme_sync.insert(
                SchemeId::new(),
                SyncDocumentMeta::local(SyncDocumentKind::Scheme),
            );
        }
        3 => {
            if let Some(id) = scheme_ids.get(rng.below(scheme_ids.len().max(1))) {
                workspace.scheme_sync.remove(id);
            }
        }
        4 => {
            if let Some(entry) = scheme_ids
                .get(rng.below(scheme_ids.len().max(1)))
                .and_then(|id| workspace.scheme_sync.get_mut(id))
            {
                entry.kind = SyncDocumentKind::Folder;
            }
        }
        5 => {
            if let Some(entry) = scheme_ids
                .get(rng.below(scheme_ids.len().max(1)))
                .and_then(|id| workspace.scheme_sync.get_mut(id))
            {
                entry.crdt = CrdtBackend::OperationLog;
            }
        }
        6 => {
            // Rebind a daily-queue scheme to the wrong document.
            let dates: Vec<NaiveDate> = workspace.daily_queue.keys().copied().collect();
            if let Some(date) = dates.get(rng.below(dates.len().max(1))) {
                let id = workspace.daily_queue[date];
                if let Some(entry) = workspace.scheme_sync.get_mut(&id) {
                    entry.id = DocumentId::new();
                }
            }
        }
        7 => {
            workspace.folder_sync.insert(
                FolderId::new(),
                SyncDocumentMeta::local(SyncDocumentKind::Folder),
            );
        }
        8 => {
            if let Some(id) = folder_ids.get(rng.below(folder_ids.len().max(1))) {
                workspace.folder_sync.remove(id);
            }
        }
        _ => {
            if let Some(entry) = folder_ids
                .get(rng.below(folder_ids.len().max(1)))
                .and_then(|id| workspace.folder_sync.get_mut(id))
            {
                entry.crdt = CrdtBackend::OperationLog;
            }
        }
    }
}

/// The predicate says "no repair needed" exactly when the repair pass changes
/// nothing, and a repaired workspace is always reported current.
fn assert_agrees(seed: u64, step: usize, workspace: &Workspace) {
    let predicted_current = workspace.sync_metadata_is_current();
    let mut repaired = workspace.clone();
    let changed = repaired.ensure_sync_metadata();

    assert_eq!(
        predicted_current, !changed,
        "seed {seed} step {step}: fast path said current={predicted_current} but the \
         repair pass changed={changed}"
    );
    if predicted_current {
        assert_eq!(
            &repaired, workspace,
            "seed {seed} step {step}: repair pass reported no change but mutated the workspace"
        );
    }
    assert!(
        repaired.sync_metadata_is_current(),
        "seed {seed} step {step}: a repaired workspace must be reported current"
    );
    // Repairing twice must be a no-op — otherwise the gate would suppress the
    // second half of a two-pass repair.
    let mut again = repaired.clone();
    assert!(
        !again.ensure_sync_metadata(),
        "seed {seed} step {step}: the repair pass is not idempotent"
    );
    assert_eq!(again, repaired);
}

#[test]
fn fast_path_agrees_with_the_repair_pass_under_random_corruption() {
    for seed in 0..400u64 {
        let mut rng = Rng(seed.wrapping_mul(0x2545_F491_4F6C_DD1D).wrapping_add(1));
        let mut workspace = seeded_workspace(&mut rng);

        // Fresh out of the builder: bindings are missing entirely.
        assert_agrees(seed, 0, &workspace);
        workspace.ensure_sync_metadata();
        assert_agrees(seed, 1, &workspace);

        for step in 2..12 {
            corrupt(&mut workspace, &mut rng);
            assert_agrees(seed, step, &workspace);
            // Repair between rounds so later corruptions start from a good base
            // roughly half the time, and stack up otherwise.
            if rng.chance(2) {
                workspace.ensure_sync_metadata();
                assert_agrees(seed, step, &workspace);
            }
        }
    }
}

/// Structural churn (adding and removing schemes, folders and daily-queue days)
/// is what leaves bindings stale in practice.
#[test]
fn fast_path_agrees_across_structural_churn() {
    for seed in 0..200u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(7));
        let mut workspace = seeded_workspace(&mut rng);
        workspace.ensure_sync_metadata();

        for step in 0..16 {
            match rng.below(6) {
                0 => {
                    let scheme = Scheme::new(format!("added-{step}"), 0);
                    workspace.schemes.insert(scheme.id, scheme);
                }
                1 => {
                    let ids: Vec<SchemeId> = workspace.schemes.keys().copied().collect();
                    if !ids.is_empty() {
                        workspace.schemes.remove(&ids[rng.below(ids.len())]);
                    }
                }
                2 => {
                    let date = date_for(rng.next_u64());
                    let id = daily_queue_scheme_id(date);
                    let mut scheme = Scheme::new("Daily", 0);
                    scheme.id = id;
                    workspace.schemes.insert(id, scheme);
                    workspace.daily_queue.insert(date, id);
                }
                3 => {
                    let dates: Vec<NaiveDate> = workspace.daily_queue.keys().copied().collect();
                    if !dates.is_empty() {
                        workspace.daily_queue.remove(&dates[rng.below(dates.len())]);
                    }
                }
                4 => {
                    let folder = knotq_model::Folder {
                        id: FolderId::new(),
                        name: format!("added-folder-{step}"),
                        parent: Some(workspace.root),
                        children: Vec::new(),
                        expanded: true,
                    };
                    workspace.folders.insert(folder.id, folder);
                }
                _ => {
                    let ids: Vec<FolderId> = workspace
                        .folders
                        .keys()
                        .copied()
                        .filter(|id| *id != workspace.root)
                        .collect();
                    if !ids.is_empty() {
                        workspace.folders.remove(&ids[rng.below(ids.len())]);
                    }
                }
            }
            assert_agrees(seed, step, &workspace);
            workspace.ensure_sync_metadata();
            assert_agrees(seed, step, &workspace);
        }
    }
}

/// The daily-queue ids are persisted on disk and agreed on between devices, so
/// their exact values are part of the on-disk/wire contract — memoizing the
/// derivation must not have perturbed them, and no future refactor may either.
/// A change here silently rebinds every existing daily-queue day to a different
/// document.
#[test]
fn daily_queue_ids_match_their_pinned_values() {
    let cases = [
        (
            2020,
            1,
            1,
            "82522b5e-b9bd-88c4-a6b8-d4e608e2c886",
            "74d93155-2e88-8f71-8d7e-940e0e51766f",
        ),
        (
            2026,
            5,
            30,
            "d439d0c5-2810-8f07-8835-31cd11d44f89",
            "698fa459-a86f-8f36-9df0-85e002dc88cc",
        ),
        (
            2026,
            8,
            16,
            "3f643817-de43-8032-86ea-a39c698157b3",
            "47fc1669-f65e-86f9-8e37-656ff7548205",
        ),
        (
            2030,
            12,
            31,
            "285759df-df5d-8d04-b818-b1e7b98bcb1a",
            "27e8c878-4fa8-84a9-b0d8-d8c183260c12",
        ),
    ];
    for (year, month, day, scheme, document) in cases {
        let date = NaiveDate::from_ymd_opt(year, month, day).unwrap();
        assert_eq!(
            daily_queue_scheme_id(date).0.to_string(),
            scheme,
            "{date}: daily-queue scheme id changed"
        );
        assert_eq!(
            daily_queue_document_id(date).0.to_string(),
            document,
            "{date}: daily-queue document id changed"
        );
    }
}

/// The memo must not change the ids' stability, distinctness, or cross-thread
/// agreement.
#[test]
fn memoized_daily_queue_ids_match_a_fresh_derivation() {
    let mut seen_scheme: HashMap<SchemeId, NaiveDate> = HashMap::new();
    let mut seen_document: HashMap<DocumentId, NaiveDate> = HashMap::new();

    for day in 0..400u64 {
        let date = NaiveDate::from_ymd_opt(2020, 1, 1)
            .unwrap()
            .checked_add_days(chrono::Days::new(day))
            .unwrap();
        let scheme = daily_queue_scheme_id(date);
        let document = daily_queue_document_id(date);

        // Repeated calls (now served from the memo) must be identical.
        for _ in 0..3 {
            assert_eq!(daily_queue_scheme_id(date), scheme);
            assert_eq!(daily_queue_document_id(date), document);
        }
        assert_ne!(
            scheme.0, document.0,
            "{date}: scheme and document ids must come from different namespaces"
        );
        if let Some(other) = seen_scheme.insert(scheme, date) {
            panic!("scheme id collision between {other} and {date}");
        }
        if let Some(other) = seen_document.insert(document, date) {
            panic!("document id collision between {other} and {date}");
        }
    }

    // Ids must be stable across threads too — the memo is thread-local, so each
    // thread derives its own and they must agree.
    let date = NaiveDate::from_ymd_opt(2026, 8, 16).unwrap();
    let (scheme, document) = (daily_queue_scheme_id(date), daily_queue_document_id(date));
    let handle = std::thread::spawn(move || (daily_queue_scheme_id(date), daily_queue_document_id(date)));
    assert_eq!(handle.join().unwrap(), (scheme, document));
}

/// `clone_without_schemes` must reproduce every field but `schemes`, so a caller
/// that refills `schemes` itself ends up with an identical workspace.
#[test]
fn clone_without_schemes_reproduces_every_other_field() {
    for seed in 0..100u64 {
        let mut rng = Rng(seed.wrapping_add(31));
        let mut workspace = seeded_workspace(&mut rng);
        workspace.ensure_sync_metadata();
        // Populate the less common fields too.
        let ids: Vec<SchemeId> = workspace.schemes.keys().copied().collect();
        if let Some(id) = ids.first() {
            workspace.recently_deleted.push(*id);
        }
        workspace.recently_deleted_folders.push(workspace.root);

        let mut rebuilt = workspace.clone_without_schemes();
        assert!(rebuilt.schemes.is_empty(), "schemes must come back empty");
        rebuilt.schemes = workspace.schemes.clone();
        assert_eq!(
            rebuilt, workspace,
            "seed {seed}: refilling schemes must reproduce the original workspace"
        );
    }
}
