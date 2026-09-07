//! End-to-end WebSocket sync tests against the REAL Cloudflare Worker running in
//! local dev mode (`wrangler dev`). These drive the FULL stack: the shared sync
//! engine (`batch_pull_and_apply`/`batch_push_pending`) over the shared
//! `knotq_sync::ws::WsClient`, over a real `tungstenite` socket, against the real
//! Durable Object WebSocket handler.
//!
//! ## How to run
//!
//! ```sh
//! # Terminal 1 — backend in test mode
//! cd app/backend/cloudflare
//! pnpm wrangler dev --local --port 8788 --var KNOTQ_TEST_MODE:1 \
//!   --persist-to .wrangler/integration-test-state
//!
//! # Terminal 2
//! export KNOTQ_SYNC_BACKEND_URL=http://127.0.0.1:8788
//! cargo test -p knotq-sync --test ws_integration -- --nocapture
//! ```
//!
//! Skips (does not fail) when `KNOTQ_SYNC_BACKEND_URL` is unset.

mod common;

use std::env;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::http_transport::{backend_bootstrap, unique_test_email};
use common::ws_transport::{connect_ws, WsTransport};
use common::TestDevice;
use knotq_model::{Workspace, WorkspaceId};
use knotq_sync::ws::{PresenceEvent, WsCallbacks, WsClient};
use std::sync::Mutex;

fn backend_url() -> Option<String> {
    match env::var("KNOTQ_SYNC_BACKEND_URL") {
        Ok(url) if !url.is_empty() => Some(url.trim_end_matches('/').to_string()),
        _ => {
            println!("[ws_integration] KNOTQ_SYNC_BACKEND_URL not set — skipping.");
            None
        }
    }
}

fn make_device(workspace_id: WorkspaceId) -> TestDevice {
    let mut base = Workspace::new();
    base.canonicalize_personal_sync_identity(workspace_id);
    base.ensure_sync_metadata();
    TestDevice::new_from_base(&base, workspace_id)
}

fn start_client(base_url: &str, token: &str, callbacks: WsCallbacks) -> Arc<WsClient> {
    connect_ws(base_url, token, callbacks)
}

fn wait_connected(_client: &WsClient) {
    // `connect_ws` already blocks until connected; kept for call-site clarity.
}

#[test]
fn ws_two_device_convergence_over_real_socket() {
    let Some(base_url) = backend_url() else {
        return;
    };
    let email = unique_test_email("ws-converge");
    let resp_a = backend_bootstrap(&base_url, &email).expect("bootstrap A");
    let resp_b = backend_bootstrap(&base_url, &email).expect("bootstrap B");
    let workspace_id: WorkspaceId = resp_a.workspace_id.parse().expect("uuid");

    let mut device_a = make_device(workspace_id);
    let mut device_b = make_device(workspace_id);

    let client_a = start_client(&base_url, &resp_a.bearer_token, WsCallbacks::noop());
    let client_b = start_client(&base_url, &resp_b.bearer_token, WsCallbacks::noop());
    wait_connected(&client_a);
    wait_connected(&client_b);
    let ws_a = WsTransport::new(Arc::clone(&client_a));
    let ws_b = WsTransport::new(Arc::clone(&client_b));

    // A creates a scheme and pushes it over the WebSocket.
    let scheme = device_a.add_scheme("WS Plan", &["alpha", "beta"]);
    device_a.try_sync_with(&ws_a).expect("A push over ws");

    // B pulls over the WebSocket and discovers it.
    device_b.try_sync_with(&ws_b).expect("B pull over ws");
    assert!(
        device_b.workspace.schemes.contains_key(&scheme),
        "device B must discover the scheme pushed over the websocket"
    );
    let items_b: Vec<String> = device_b.workspace.schemes[&scheme]
        .items
        .iter()
        .map(|i| i.text())
        .collect();
    assert!(
        items_b.iter().any(|t| t == "alpha") && items_b.iter().any(|t| t == "beta"),
        "device B must see both items over ws; got {items_b:?}"
    );

    // B edits and pushes; A pulls and converges — all over the socket.
    device_b.append_line(scheme, "gamma");
    device_b.try_sync_with(&ws_b).expect("B push over ws");
    device_a.try_sync_with(&ws_a).expect("A pull over ws");
    let items_a: Vec<String> = device_a.workspace.schemes[&scheme]
        .items
        .iter()
        .map(|i| i.text())
        .collect();
    assert!(
        items_a.iter().any(|t| t == "gamma"),
        "device A must converge to gamma over ws; got {items_a:?}"
    );
}

#[test]
fn ws_push_broadcasts_changed_to_other_socket() {
    let Some(base_url) = backend_url() else {
        return;
    };
    let email = unique_test_email("ws-changed");
    let resp_a = backend_bootstrap(&base_url, &email).expect("bootstrap A");
    let resp_b = backend_bootstrap(&base_url, &email).expect("bootstrap B");
    let workspace_id: WorkspaceId = resp_a.workspace_id.parse().expect("uuid");

    let mut device_a = make_device(workspace_id);

    let changed = Arc::new(AtomicUsize::new(0));
    let client_a = start_client(&base_url, &resp_a.bearer_token, WsCallbacks::noop());
    let client_b = start_client(
        &base_url,
        &resp_b.bearer_token,
        WsCallbacks {
            on_changed: {
                let changed = Arc::clone(&changed);
                Box::new(move || {
                    changed.fetch_add(1, Ordering::SeqCst);
                })
            },
            on_presence: Box::new(|_| {}),
            on_connect: Box::new(|| {}),
        },
    );
    wait_connected(&client_a);
    wait_connected(&client_b);
    // B must have issued at least one request so the DO learns its replica before
    // we assert it is nudged — do an initial pull.
    let ws_b = WsTransport::new(Arc::clone(&client_b));
    let mut device_b = make_device(workspace_id);
    device_b.try_sync_with(&ws_b).expect("B initial pull");

    // A pushes a change; the DO should broadcast `changed` to B's socket.
    let ws_a = WsTransport::new(Arc::clone(&client_a));
    device_a.add_scheme("Broadcast", &["x"]);
    device_a.try_sync_with(&ws_a).expect("A push");

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && changed.load(Ordering::SeqCst) == 0 {
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(
        changed.load(Ordering::SeqCst) >= 1,
        "device B's socket should receive a `changed` nudge after A pushes"
    );
}

#[test]
fn ws_presence_relays_between_sockets_over_real_backend() {
    let Some(base_url) = backend_url() else {
        return;
    };
    let email = unique_test_email("ws-presence");
    let resp_a = backend_bootstrap(&base_url, &email).expect("bootstrap A");
    let resp_b = backend_bootstrap(&base_url, &email).expect("bootstrap B");

    let received: Arc<Mutex<Vec<PresenceEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let client_a = start_client(&base_url, &resp_a.bearer_token, WsCallbacks::noop());
    let client_b = start_client(
        &base_url,
        &resp_b.bearer_token,
        WsCallbacks {
            on_changed: Box::new(|| {}),
            on_presence: {
                let received = Arc::clone(&received);
                Box::new(move |event| received.lock().unwrap().push(event))
            },
            on_connect: Box::new(|| {}),
        },
    );
    wait_connected(&client_a);
    wait_connected(&client_b);

    // A broadcasts an ephemeral cursor; B's socket should relay it (never persisted).
    client_a
        .send_presence(serde_json::json!({ "item": "abc", "caret": 12 }))
        .expect("send presence");

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && received.lock().unwrap().is_empty() {
        std::thread::sleep(Duration::from_millis(25));
    }
    let events = received.lock().unwrap();
    assert_eq!(
        events.len(),
        1,
        "B should receive exactly one presence frame"
    );
    assert_eq!(
        events[0].data.as_ref().unwrap()["caret"],
        12,
        "presence payload must round-trip"
    );
}

// ---------------------------------------------------------------------------
// Scenario + fuzz coverage over the real WebSocket
//
// `Harness::new_ws` runs the SAME scenario functions the in-memory and HTTP
// suites use, but every `sync` goes over a real `WsClient` -> tungstenite ->
// Durable Object socket. Nothing here re-implements the client: it is the
// production engine over the production transport against the real worker.
// ---------------------------------------------------------------------------

/// Bootstrap `n` devices sharing one workspace and return their tokens + id.
fn bootstrap_ws_tokens(base_url: &str, label: &str, n: usize) -> (Vec<String>, WorkspaceId) {
    let email = unique_test_email(label);
    let mut tokens = Vec::new();
    let mut workspace_id_str = String::new();
    for i in 0..n {
        let resp =
            backend_bootstrap(base_url, &email).unwrap_or_else(|e| panic!("bootstrap {i}: {e}"));
        if workspace_id_str.is_empty() {
            workspace_id_str = resp.workspace_id.clone();
        }
        tokens.push(resp.bearer_token);
    }
    (
        tokens,
        workspace_id_str.parse().expect("workspace_id uuid"),
    )
}

/// Bootstrap `n` devices sharing one workspace and return a WS-backed harness.
fn bootstrap_ws_harness(base_url: &str, label: &str, n: usize) -> common::Harness {
    let (tokens, workspace_id) = bootstrap_ws_tokens(base_url, label, n);
    common::Harness::new_ws(base_url, workspace_id, tokens)
}

/// Same, but pull/push alternate WS / HTTP per request (mid-cycle fallback).
fn bootstrap_ws_mixed_harness(base_url: &str, label: &str, n: usize) -> common::Harness {
    let (tokens, workspace_id) = bootstrap_ws_tokens(base_url, label, n);
    common::Harness::new_ws_mixed(base_url, workspace_id, tokens)
}

fn device_fingerprint(dev: &TestDevice) -> Vec<String> {
    let mut out: Vec<String> = dev
        .workspace
        .schemes
        .values()
        .map(|s| {
            format!(
                "{}=[{}]",
                s.name,
                s.items.iter().map(|i| i.text()).collect::<Vec<_>>().join(",")
            )
        })
        .collect();
    out.sort();
    out
}

fn fuzz_env(default_seeds: u64, default_steps: usize) -> (u64, usize) {
    let seeds = env::var("KNOTQ_WS_FUZZ_SEEDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default_seeds);
    let steps = env::var("KNOTQ_WS_FUZZ_STEPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default_steps);
    (seeds, steps)
}

#[test]
fn ws_scenario_g2_daily_queue_direct_creation() {
    let Some(base_url) = backend_url() else {
        return;
    };
    let mut h = bootstrap_ws_harness(&base_url, "ws-g2", 2);
    common::scenarios::scenario_g2_daily_queue_direct_creation(&mut h);
}

#[test]
fn ws_scenario_e_long_offline_divergence() {
    let Some(base_url) = backend_url() else {
        return;
    };
    let mut h = bootstrap_ws_harness(&base_url, "ws-e-offline", 2);
    common::scenarios::scenario_e_long_offline_divergence(&mut h);
}

#[test]
fn ws_scenario_f_offline_restart_combo() {
    let Some(base_url) = backend_url() else {
        return;
    };
    let mut h = bootstrap_ws_harness(&base_url, "ws-f-restart", 2);
    common::scenarios::scenario_f_offline_restart_combo(&mut h);
}

#[test]
fn ws_scenario_g_daily_queue_conflicts() {
    let Some(base_url) = backend_url() else {
        return;
    };
    let mut h = bootstrap_ws_harness(&base_url, "ws-g", 2);
    common::scenarios::scenario_g_daily_queue_conflicts(&mut h);
}

#[test]
fn ws_scenario_m2_carryover_concurrent_shared_today() {
    let Some(base_url) = backend_url() else {
        return;
    };
    let mut h = bootstrap_ws_harness(&base_url, "ws-m2", 2);
    common::scenarios::scenario_m2_carryover_concurrent_shared_today(&mut h);
}

/// The randomized fuzz scenario over a real socket. Deterministic per seed;
/// crank with `KNOTQ_WS_FUZZ_SEEDS` / `KNOTQ_WS_FUZZ_STEPS`. Kept modest by
/// default because every op is a real round-trip to `wrangler dev`.
#[test]
fn ws_scenario_l_randomized_fuzz() {
    let Some(base_url) = backend_url() else {
        return;
    };
    let (seeds, steps) = fuzz_env(3, 40);
    for seed in 0..seeds {
        let mut h = bootstrap_ws_harness(&base_url, &format!("ws-fuzz-{seed}"), 3);
        common::scenarios::scenario_l_randomized_fuzz(&mut h, seed, steps);
    }
}

/// The same fuzz, but every request alternates WS / HTTP — so within a single
/// sync cycle the pull and the push routinely go over different transports, and
/// a document seen over one is merged from bytes fetched over the other. This is
/// the `FallbackTransport` reality (socket drops mid-cycle, next request is
/// HTTP, then it reconnects).
#[test]
fn ws_scenario_l_randomized_fuzz_mixed_transport() {
    let Some(base_url) = backend_url() else {
        return;
    };
    let (seeds, steps) = fuzz_env(3, 40);
    for seed in 0..seeds {
        let mut h = bootstrap_ws_mixed_harness(&base_url, &format!("ws-mixed-{seed}"), 3);
        common::scenarios::scenario_l_randomized_fuzz(&mut h, seed, steps);
    }
}

/// Account-HOPPING fuzz over the real socket: several devices, several
/// workspaces, a random sequence of {edit, sync, switch account}, all over
/// WebSocket (a fresh `WsClient` per account). After settling, every device on
/// an account converges with a freshly-signed-in device — no silent loss — and
/// the backend never rejected a push. This is the `sync_property_model` account
/// churn, end to end over the production transport against the real worker.
#[test]
fn ws_account_hopping_fuzz_converges() {
    let Some(base_url) = backend_url() else {
        return;
    };
    // Its own knobs (`KNOTQ_WS_HOP_*`): every step is a real round trip and an
    // account switch re-opens a socket, so this is the slowest WS test. Small by
    // default; the nightly raises it.
    let seeds = env::var("KNOTQ_WS_HOP_SEEDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1u64);
    let steps = env::var("KNOTQ_WS_HOP_STEPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(16usize);
    let n_workspaces = 3usize;
    let n_devices = 3usize;

    for seed in 0..seeds {
        // Bootstrap every token for a workspace in ONE call (they share an email
        // and therefore the workspace id). The last token per workspace is the
        // "fresh signed-in device" used for the no-silent-loss check.
        let mut ws_ids = Vec::new();
        let mut tokens: Vec<Vec<String>> = Vec::new(); // [workspace][device.. , fresh]
        for w in 0..n_workspaces {
            let (t, id) = bootstrap_ws_tokens(
                &base_url,
                &format!("ws-hop-{seed}-{w}"),
                n_devices + 1,
            );
            ws_ids.push(id);
            tokens.push(t);
        }

        struct Dev {
            inner: TestDevice,
            account: usize,
            client: Arc<WsClient>,
        }
        let mut devices: Vec<Dev> = (0..n_devices)
            .map(|d| {
                let account = d % n_workspaces;
                Dev {
                    inner: make_device(ws_ids[account]),
                    account,
                    client: start_client(
                        &base_url,
                        &tokens[account][d],
                        WsCallbacks::noop(),
                    ),
                }
            })
            .collect();

        let mut rng = seed.wrapping_mul(0x9e37_79b9) ^ 0xdead_beef;
        let next = |rng: &mut u64| {
            *rng ^= *rng << 13;
            *rng ^= *rng >> 7;
            *rng ^= *rng << 17;
            *rng
        };

        for step in 0..steps {
            let r = next(&mut rng);
            let d = (r % n_devices as u64) as usize;
            match (r >> 8) % 5 {
                0 | 1 => {
                    let name = format!("s{seed}-d{d}-{step}");
                    let scheme = devices[d].inner.add_scheme(&name, &["x"]);
                    let _ = scheme;
                    let account = devices[d].account;
                    let transport = WsTransport::new(Arc::clone(&devices[d].client));
                    let _ = devices[d].inner.try_sync_with(&transport);
                    let _ = account;
                }
                2 => {
                    let transport = WsTransport::new(Arc::clone(&devices[d].client));
                    let _ = devices[d].inner.try_sync_with(&transport);
                }
                _ => {
                    // Switch device `d` to a different workspace.
                    let target = ((r >> 16) as usize + 1) % n_workspaces;
                    if target != devices[d].account {
                        devices[d].client.shutdown();
                        devices[d].inner.switch_account(ws_ids[target], &base_url);
                        devices[d].account = target;
                        devices[d].client = start_client(
                            &base_url,
                            &tokens[target][d],
                            WsCallbacks::noop(),
                        );
                        let transport = WsTransport::new(Arc::clone(&devices[d].client));
                        let _ = devices[d].inner.try_sync_with(&transport);
                    }
                }
            }
        }

        // Settle: an account switch queues a full-snapshot reseed that needs
        // several round trips to propagate, so sync every device generously
        // (idempotent — extra rounds are harmless).
        for _ in 0..(n_devices * 6 + 16) {
            for dev in &mut devices {
                let transport = WsTransport::new(Arc::clone(&dev.client));
                let _ = dev.inner.try_sync_with(&transport);
            }
        }

        // Per account: every device on it converges with a fresh signed-in
        // device, and nothing is stuck unpushed (the wedge symptom).
        for (w, ws_id) in ws_ids.iter().enumerate() {
            let on_account: Vec<&Dev> = devices.iter().filter(|d| d.account == w).collect();
            let Some(first) = on_account.first() else {
                continue;
            };
            for dev in &on_account {
                assert!(
                    dev.inner.is_fully_pushed(),
                    "seed {seed}: a device on account {w} has stuck pending (wedge)"
                );
            }
            let mut fresh = make_device(*ws_id);
            let fresh_client =
                start_client(&base_url, &tokens[w][n_devices], WsCallbacks::noop());
            for _ in 0..4 {
                let _ = fresh.try_sync_with(&WsTransport::new(Arc::clone(&fresh_client)));
            }
            fresh_client.shutdown();
            assert!(
                first.inner.converges_with(&fresh),
                "seed {seed}: account {w} devices diverge from a fresh signed-in device\n  device: {:?}\n  fresh:  {:?}",
                device_fingerprint(&first.inner),
                device_fingerprint(&fresh),
            );
            for dev in &on_account[1..] {
                assert!(
                    first.inner.converges_with(&dev.inner),
                    "seed {seed}: two devices on account {w} diverge"
                );
            }
        }
        for dev in devices {
            dev.client.shutdown();
        }
    }
}

/// A real account switch over the WebSocket: a device signs out of workspace A
/// and into workspace B (different bearer token, different DO), and its A content
/// must reach B by an idempotent full-snapshot re-seed — with the backend never
/// rejecting a push. This is the sign-in/sign-out path from
/// `account_switch_scenarios.rs`, but end to end over the real socket stack.
#[test]
fn ws_account_switch_carries_content_over_real_socket() {
    let Some(base_url) = backend_url() else {
        return;
    };
    let email_a = unique_test_email("ws-acct-a");
    let email_b = unique_test_email("ws-acct-b");
    let resp_a = backend_bootstrap(&base_url, &email_a).expect("bootstrap A");
    let resp_b = backend_bootstrap(&base_url, &email_b).expect("bootstrap B");
    let resp_b_puller = backend_bootstrap(&base_url, &email_b).expect("bootstrap B puller");
    let ws_a: WorkspaceId = resp_a.workspace_id.parse().expect("uuid A");
    let ws_b: WorkspaceId = resp_b.workspace_id.parse().expect("uuid B");
    assert_ne!(ws_a, ws_b, "the two accounts must be different workspaces");

    // Device lives on account A, creates content, pushes over WS-A.
    let mut device = make_device(ws_a);
    let client_a = start_client(&base_url, &resp_a.bearer_token, WsCallbacks::noop());
    let scheme = device.add_scheme("Carried plan", &["from account A"]);
    device
        .try_sync_with(&WsTransport::new(Arc::clone(&client_a)))
        .expect("push on A over ws");
    client_a.shutdown();

    // Sign out of A, into B (adopt B's canonical identity + reset cursors), then
    // sync over WS-B. The A content re-seeds into B.
    device.switch_account(ws_b, &base_url);
    let client_b = start_client(&base_url, &resp_b.bearer_token, WsCallbacks::noop());
    device
        .try_sync_with(&WsTransport::new(Arc::clone(&client_b)))
        .expect("sync on B over ws after switch");

    // A fresh device on B must see exactly that content — no silent loss.
    let mut puller = make_device(ws_b);
    let client_b_puller =
        start_client(&base_url, &resp_b_puller.bearer_token, WsCallbacks::noop());
    puller
        .try_sync_with(&WsTransport::new(Arc::clone(&client_b_puller)))
        .expect("fresh B device pull over ws");

    let by_name = |dev: &TestDevice| -> Vec<String> {
        dev.workspace
            .schemes
            .values()
            .filter(|s| s.name == "Carried plan")
            .flat_map(|s| s.items.iter().map(|i| i.text()))
            .collect()
    };
    let _ = scheme;
    assert_eq!(
        by_name(&device),
        vec!["from account A".to_string()],
        "the switched device keeps its content on account B"
    );
    assert_eq!(
        by_name(&puller),
        vec!["from account A".to_string()],
        "a fresh device on account B sees the carried content"
    );
}
