//! Does an *agent's* edit converge the way a user's does?
//!
//! The MCP server hands every write to the same `apply` path a keystroke takes,
//! so in principle nothing here can behave differently. This asserts it rather
//! than assuming it, because "in principle" is exactly how the earlier
//! divergence bugs got in: the CRDT layer is only commutative if every writer
//! goes through it the same way, and a new writer is precisely the kind of
//! thing that quietly grows its own path.
//!
//! Each test drives real MCP tool calls — parsed from JSON, dispatched by name,
//! the emitted `Command` applied with `CommandOrigin::Agent` — across two
//! devices, then syncs and asserts the devices agree.

mod common;

use common::{TestDevice, TestServer};
use knotq_commands::{Command, CommandOrigin, WorkspaceCommandExt};
use knotq_index::IndexedWorkspace;
use knotq_mcp::{call_tool, Outcome, ToolContext};
use knotq_model::{SchemeId, TimeFormat, Workspace, WorkspaceId};
use knotq_sync::WorkspaceCrdtChangeSet;
use serde_json::{json, Value};

fn fresh_device(account: WorkspaceId) -> TestDevice {
    let mut base = Workspace::new();
    base.canonicalize_personal_sync_identity(account);
    TestDevice::new_from_base(&base, account)
}

fn now() -> chrono::DateTime<chrono::Utc> {
    use chrono::TimeZone;
    chrono::Utc.with_ymd_and_hms(2026, 9, 1, 12, 0, 0).unwrap()
}

fn today() -> chrono::NaiveDate {
    chrono::NaiveDate::from_ymd_opt(2026, 9, 1).unwrap()
}

/// Which CRDT documents a command's effects live in.
///
/// The desktop derives this the same way when it signals the sync service; the
/// mapping is repeated here rather than shared because getting it *wrong* is
/// one of the failures this file is meant to catch — a test that reused the
/// production mapping would agree with a bug in it.
fn changes_for(command: &Command) -> WorkspaceCrdtChangeSet {
    let mut set = WorkspaceCrdtChangeSet::default();
    fn walk(command: &Command, set: &mut WorkspaceCrdtChangeSet) {
        let touched = match command {
            Command::Batch(inner) => {
                for c in inner {
                    walk(c, set);
                }
                return;
            }
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
            | Command::ReorderItem { scheme, .. } => Some(*scheme),
            Command::RenameScheme { id, .. }
            | Command::SetSchemeColor { id, .. }
            | Command::SetSchemeGsync { id, .. }
            | Command::SetSchemeSource { id, .. }
            | Command::DeleteScheme { id }
            | Command::PermanentlyDeleteScheme { id } => Some(*id),
            _ => None,
        };
        // Anything structural also moves the workspace document, and a
        // conservative extra touch costs a no-op update, never a lost edit.
        *set = std::mem::take(set).workspace();
        if let Some(scheme) = touched {
            *set = std::mem::take(set).touch_scheme(scheme);
        }
    }
    walk(command, &mut set);
    set
}

/// Run one MCP tool call against a device and apply whatever it produced,
/// exactly as `mcp_service` does on the desktop's main thread.
fn agent_call(device: &mut TestDevice, name: &str, args: Value) -> Value {
    let indexed = IndexedWorkspace::build(device.workspace.clone());
    let ctx = ToolContext::new(&indexed, now(), today(), TimeFormat::default(), false);
    let outcome = call_tool(name, Some(&args), &ctx)
        .unwrap_or_else(|e| panic!("tool `{name}` failed: {e}"));
    match outcome {
        Outcome::Read(v) | Outcome::Unchanged(v) => v,
        Outcome::Write { command, response } => {
            let changes = changes_for(&command);
            device
                .workspace
                .apply_with_origin(command, CommandOrigin::Agent)
                .unwrap_or_else(|e| panic!("applying `{name}` failed: {e}"));
            device.record_changes(changes);
            response
        }
    }
}

fn texts(device: &TestDevice, scheme: SchemeId) -> Vec<String> {
    device.scheme_item_texts(scheme)
}

#[test]
fn an_agent_edit_reaches_another_device_unchanged() {
    let account = WorkspaceId::new();
    let server = TestServer::default();
    let mut agent_device = fresh_device(account);
    let mut other = fresh_device(account);

    let scheme = agent_device.add_scheme("Plan", &["existing line"]);
    agent_device.try_sync(&server).unwrap();
    other.try_sync(&server).unwrap();

    agent_call(
        &mut agent_device,
        "add_item",
        json!({ "scheme_id": scheme.to_string(), "text": "added by the agent" }),
    );
    agent_device.try_sync(&server).unwrap();
    other.try_sync(&server).unwrap();

    assert_eq!(
        texts(&other, scheme),
        vec!["existing line", "added by the agent"]
    );
    assert!(agent_device.converges_with(&other));
}

/// The case that actually matters: the user is typing on one device while the
/// agent writes on another. Both edits must survive.
#[test]
fn concurrent_agent_and_human_edits_both_survive() {
    let account = WorkspaceId::new();
    let server = TestServer::default();
    let mut agent_device = fresh_device(account);
    let mut human = fresh_device(account);

    let scheme = agent_device.add_scheme("Plan", &["shared line"]);
    agent_device.try_sync(&server).unwrap();
    human.try_sync(&server).unwrap();

    // Neither has seen the other yet.
    agent_call(
        &mut agent_device,
        "add_item",
        json!({ "scheme_id": scheme.to_string(), "text": "from the agent" }),
    );
    human.append_line(scheme, "from the human");

    agent_device.try_sync(&server).unwrap();
    human.try_sync(&server).unwrap();
    agent_device.try_sync(&server).unwrap();
    human.try_sync(&server).unwrap();

    let settled = texts(&human, scheme);
    assert!(settled.contains(&"from the agent".to_string()), "{settled:?}");
    assert!(settled.contains(&"from the human".to_string()), "{settled:?}");
    assert_eq!(settled.len(), 3);
    assert!(agent_device.converges_with(&human));
}

/// Two agents completing the same task at once. The tool is a *set*, not a
/// toggle, precisely so this does not end with the task open again.
#[test]
fn two_agents_completing_the_same_line_leaves_it_completed() {
    let account = WorkspaceId::new();
    let server = TestServer::default();
    let mut a = fresh_device(account);
    let mut b = fresh_device(account);

    let scheme = a.add_scheme("Plan", &["do the thing"]);
    a.try_sync(&server).unwrap();
    b.try_sync(&server).unwrap();
    let item = a.workspace.schemes[&scheme].items[0].id;

    let args = json!({
        "scheme_id": scheme.to_string(),
        "item_id": item.to_string(),
        "completed": true,
    });
    agent_call(&mut a, "set_item_completed", args.clone());
    agent_call(&mut b, "set_item_completed", args);

    a.try_sync(&server).unwrap();
    b.try_sync(&server).unwrap();
    a.try_sync(&server).unwrap();
    b.try_sync(&server).unwrap();

    for device in [&a, &b] {
        assert!(
            device.workspace.schemes[&scheme].items[0]
                .single_state()
                .is_done(),
            "two agents agreeing must not cancel each other out"
        );
    }
    assert!(a.converges_with(&b));
}

/// The retry case, across a sync boundary: the agent's first call landed and
/// synced, the response was lost, and the retry arrives on a device that has
/// already seen the edit.
#[test]
fn a_retried_add_does_not_duplicate_across_devices() {
    let account = WorkspaceId::new();
    let server = TestServer::default();
    let mut a = fresh_device(account);
    let mut b = fresh_device(account);

    let scheme = a.add_scheme("Plan", &[]);
    a.try_sync(&server).unwrap();
    b.try_sync(&server).unwrap();

    let item_id = knotq_model::ItemId::new();
    let args = json!({
        "scheme_id": scheme.to_string(),
        "text": "exactly once",
        "item_id": item_id.to_string(),
    });

    agent_call(&mut a, "add_item", args.clone());
    a.try_sync(&server).unwrap();
    b.try_sync(&server).unwrap();

    // The retry lands on the *other* device, which already has the line.
    let response = agent_call(&mut b, "add_item", args);
    assert_eq!(response["created"], json!(false));

    b.try_sync(&server).unwrap();
    a.try_sync(&server).unwrap();

    assert_eq!(texts(&a, scheme), vec!["exactly once"]);
    assert!(a.converges_with(&b));
}

#[test]
fn a_long_mixed_run_of_agent_and_human_edits_converges() {
    let account = WorkspaceId::new();
    let server = TestServer::default();
    let mut agent_device = fresh_device(account);
    let mut human = fresh_device(account);

    let scheme = agent_device.add_scheme("Plan", &["seed"]);
    agent_device.try_sync(&server).unwrap();
    human.try_sync(&server).unwrap();

    for round in 0..12 {
        agent_call(
            &mut agent_device,
            "add_item",
            json!({
                "scheme_id": scheme.to_string(),
                "text": format!("agent {round}"),
            }),
        );
        human.append_line(scheme, &format!("human {round}"));

        // Sync only sometimes, so several rounds of both sides' edits pile up
        // unseen before they meet.
        if round % 3 == 2 {
            agent_device.try_sync(&server).unwrap();
            human.try_sync(&server).unwrap();
            agent_device.try_sync(&server).unwrap();
        }
    }

    for _ in 0..3 {
        agent_device.try_sync(&server).unwrap();
        human.try_sync(&server).unwrap();
    }

    let settled = texts(&human, scheme);
    for round in 0..12 {
        assert!(
            settled.contains(&format!("agent {round}")),
            "lost agent {round}: {settled:?}"
        );
        assert!(
            settled.contains(&format!("human {round}")),
            "lost human {round}: {settled:?}"
        );
    }
    assert!(agent_device.converges_with(&human));
    assert!(agent_device.is_fully_pushed());
}

/// Structural writes (folders, schemes) live in the workspace document rather
/// than a scheme document, and have their own history of divergence.
#[test]
fn agent_created_schemes_and_folders_converge() {
    let account = WorkspaceId::new();
    let server = TestServer::default();
    let mut a = fresh_device(account);
    let mut b = fresh_device(account);
    a.try_sync(&server).unwrap();
    b.try_sync(&server).unwrap();

    agent_call(&mut a, "create_folder", json!({ "name": "Projects" }));
    a.try_sync(&server).unwrap();
    b.try_sync(&server).unwrap();

    let folders = agent_call(&mut a, "list_schemes", json!({}));
    let folder_id = folders["folders"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == "Projects")
        .expect("the folder the agent just made")["id"]
        .as_str()
        .unwrap()
        .to_string();

    agent_call(
        &mut a,
        "create_scheme",
        json!({ "name": "Roadmap", "folder_id": folder_id }),
    );
    a.try_sync(&server).unwrap();
    b.try_sync(&server).unwrap();

    assert!(b.has_folder_named("Projects"));
    assert!(b.has_scheme_named("Roadmap"));
    assert!(a.converges_with(&b));
}

/// Archiving is a workspace-document edit with a separate restore path, and it
/// is the destructive-looking operation an agent is most likely to perform.
#[test]
fn an_agent_archiving_a_scheme_converges_and_is_still_restorable() {
    let account = WorkspaceId::new();
    let server = TestServer::default();
    let mut a = fresh_device(account);
    let mut b = fresh_device(account);

    let scheme = a.add_scheme("Old plan", &["a line"]);
    a.try_sync(&server).unwrap();
    b.try_sync(&server).unwrap();

    agent_call(
        &mut a,
        "delete_scheme",
        json!({ "scheme_id": scheme.to_string() }),
    );
    a.try_sync(&server).unwrap();
    b.try_sync(&server).unwrap();

    assert!(b.scheme_is_archived(scheme));
    // Archived, not destroyed: the content is still there for the user to restore.
    assert_eq!(texts(&b, scheme), vec!["a line"]);
    assert!(a.converges_with(&b));
}

/// A fresh device signing in must see the agent's work — the server has to hold
/// materializable state, not just a delta the original device happened to have.
#[test]
fn a_new_device_sees_everything_the_agent_wrote() {
    let account = WorkspaceId::new();
    let server = TestServer::default();
    let mut a = fresh_device(account);

    let scheme = a.add_scheme("Plan", &[]);
    for i in 0..5 {
        agent_call(
            &mut a,
            "add_item",
            json!({ "scheme_id": scheme.to_string(), "text": format!("line {i}") }),
        );
    }
    a.try_sync(&server).unwrap();

    let mut newcomer = fresh_device(account);
    newcomer.try_sync(&server).unwrap();

    assert_eq!(
        texts(&newcomer, scheme),
        (0..5).map(|i| format!("line {i}")).collect::<Vec<_>>()
    );
    assert!(a.converges_with(&newcomer));
}

/// An agent restarting mid-conversation must not resurrect state. This is the
/// same restart path a real desktop takes when the app is quit and reopened
/// with unsynced agent edits still pending.
#[test]
fn agent_edits_survive_a_restart_before_they_are_pushed() {
    let account = WorkspaceId::new();
    let server = TestServer::default();
    let mut a = fresh_device(account);
    let mut b = fresh_device(account);

    let scheme = a.add_scheme("Plan", &[]);
    a.try_sync(&server).unwrap();
    b.try_sync(&server).unwrap();

    agent_call(
        &mut a,
        "add_item",
        json!({ "scheme_id": scheme.to_string(), "text": "written before the crash" }),
    );
    // Never pushed.
    a.restart();
    a.try_sync(&server).unwrap();
    b.try_sync(&server).unwrap();

    assert_eq!(texts(&b, scheme), vec!["written before the crash"]);
    assert!(a.converges_with(&b));
}
