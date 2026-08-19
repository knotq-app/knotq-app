//! Per-keystroke main-thread cost against a real workspace.
//!
//! Ignored by default: point `KNOTQ_PROBE_DIR` at a *copy* of a workspace
//! directory and run
//! `cargo test -p knotq-state --test edit_cost_probe -- --ignored --nocapture`.

use std::time::Instant;

use knotq_commands::Command;
use knotq_model::{AppSettings, ItemId, SchemeId};
use knotq_state::AppState;

fn ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

#[test]
#[ignore]
fn probe_edit_costs() {
    let dir = std::path::PathBuf::from(
        std::env::var("KNOTQ_PROBE_DIR").expect("set KNOTQ_PROBE_DIR to a workspace copy"),
    );
    let workspace_path = dir.join("workspace.json");
    let workspace = knotq_storage_json::load_workspace(&workspace_path)
        .expect("load workspace")
        .expect("workspace present");
    let crdt_states = knotq_storage_json::load_crdt_state(&workspace_path).expect("load crdt");
    let today = chrono::Local::now().date_naive();

    let start = Instant::now();
    let mut state = AppState::new(
        workspace.clone(),
        AppSettings::default(),
        today,
        today,
        false,
        crdt_states,
        0,
    );
    println!("AppState::new (CRDT restore)  {:8.1} ms", ms(start));

    // The biggest scheme is the worst case for a keystroke.
    let (scheme_id, item_id, text): (SchemeId, ItemId, String) = workspace
        .schemes
        .values()
        // Imported calendars are read-only, so they would reject every edit.
        .filter(|scheme| !scheme.is_read_only())
        .max_by_key(|scheme| scheme.items.len())
        .and_then(|scheme| {
            scheme
                .items
                .first()
                .map(|item| (scheme.id, item.id, item.text().to_string()))
        })
        .expect("a scheme with an item");
    let items = workspace.schemes[&scheme_id].items.len();

    let start = Instant::now();
    let _ = state.indexed();
    println!("indexed() first build         {:8.1} ms", ms(start));

    let mut total = 0.0;
    let keystrokes = 20;
    for index in 0..keystrokes {
        let mut next = text.clone();
        next.push_str(&format!("x{index}"));
        let start = Instant::now();
        let receipt = state.apply_editor_command(Command::UpdateItemText {
            scheme: scheme_id,
            item: item_id,
            text: next,
        });
        total += ms(start);
        assert!(receipt.is_some(), "edit {index} was rejected");
    }
    println!(
        "apply_editor_command(text)    {:8.2} ms   (mean of {keystrokes}, scheme has {items} items)",
        total / keystrokes as f64
    );

    let start = Instant::now();
    let _ = state.indexed();
    println!("indexed() after edits         {:8.1} ms", ms(start));

    // Does a keystroke cost scale with the size of the scheme being typed in?
    let mut editable: Vec<_> = workspace
        .schemes
        .values()
        .filter(|scheme| !scheme.is_read_only() && !scheme.items.is_empty())
        .collect();
    editable.sort_by_key(|scheme| std::cmp::Reverse(scheme.items.len()));
    for scheme in editable.iter().take(4) {
        let item = scheme.items[0].id;
        let base = scheme.items[0].text().to_string();
        let mut total = 0.0;
        let runs = 10;
        for index in 0..runs {
            let mut next = base.clone();
            next.push_str(&format!("y{index}"));
            let start = Instant::now();
            let _ = state.apply_prechecked_local_command(
                Command::UpdateItemText {
                    scheme: scheme.id,
                    item,
                    text: next,
                },
                knotq_commands::CommandOrigin::User,
            );
            total += ms(start);
        }
        println!(
            "  keystroke in {:4} items      {:8.2} ms   (app path)",
            scheme.items.len(),
            total / runs as f64
        );
    }

    // The same edit applied straight to the workspace, with no CRDT bookkeeping,
    // to separate the command from the sync work it triggers.
    {
        let scheme = editable[0];
        let mut plain = workspace.clone();
        let item = scheme.items[0].id;
        let base = scheme.items[0].text().to_string();
        let mut total = 0.0;
        let runs = 10;
        for index in 0..runs {
            let mut next = base.clone();
            next.push_str(&format!("z{index}"));
            let start = Instant::now();
            let _ = knotq_commands::WorkspaceCommandExt::apply(
                &mut plain,
                Command::UpdateItemText {
                    scheme: scheme.id,
                    item,
                    text: next,
                },
            );
            total += ms(start);
        }
        println!(
            "  command only, no CRDT        {:8.2} ms   ({} items)",
            total / runs as f64,
            scheme.items.len()
        );
    }

    let start = Instant::now();
    let states = state.crdt_document_states();
    println!(
        "crdt_document_states() cold   {:8.1} ms   ({} docs)",
        ms(start),
        states.len()
    );
    drop(states);

    let start = Instant::now();
    let states = state.crdt_document_states();
    println!("crdt_document_states() warm   {:8.2} ms   (nothing changed)", ms(start));
    drop(states);

    // What a save actually meets: one scheme edited since the last save.
    let mut next = text.clone();
    next.push_str("edited");
    state.apply_editor_command(Command::UpdateItemText {
        scheme: scheme_id,
        item: item_id,
        text: next,
    });
    let start = Instant::now();
    let states = state.crdt_document_states();
    println!("crdt_document_states() 1 edit {:8.2} ms   (per save while typing)", ms(start));
    drop(states);

    let start = Instant::now();
    let edits = state.pending_crdt_edits();
    println!(
        "pending_crdt_edits()          {:8.1} ms   ({} edits)",
        ms(start),
        edits.len()
    );
}
