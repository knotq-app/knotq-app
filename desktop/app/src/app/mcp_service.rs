//! The local MCP server: an AI assistant reading and editing this workspace.
//!
//! # Why it lives inside the app
//!
//! Every write an agent makes goes through [`KnotQApp::apply`] on the GPUI main
//! thread — the same call a keystroke makes. That is not an implementation
//! detail, it is the whole design:
//!
//! * `apply` is where the invariants run, where the undo entry is pushed, where
//!   notifications are reconciled and where the CRDT edit is recorded. A writer
//!   that reached around it would produce edits that do not undo, do not
//!   reschedule notifications, and — the expensive one — do not merge, because
//!   the CRDT is only commutative if every writer feeds it the same way.
//! * Being on the main thread means an agent edit is serialised against the
//!   user's own edits by construction. There is no lock to forget and no
//!   window in which two writers hold different ideas of the workspace.
//!
//! So the server's job is narrow: accept a connection, decide whether it is
//! allowed to talk to us at all, turn its JSON-RPC into a tool call, hop to the
//! main thread for exactly as long as it takes to evaluate and apply, and hop
//! back. Everything about *what* the tools do lives in `knotq-mcp`, which is
//! pure and tested on its own.
//!
//! # Threading
//!
//! ```text
//!   listener thread ──accept──▶ connection thread
//!                                     │ (blocking send)
//!                              async_channel<McpJob>
//!                                     │
//!                       GPUI task ────┴──▶ weak.update ──▶ main thread
//!                                     │                      │
//!                              sync_channel reply ◀──────────┘
//! ```
//!
//! The connection thread blocks on the reply, so a client sees one response per
//! request with no ordering games. The main thread does only the evaluation.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use async_channel::{Receiver, Sender};
use chrono::{Local, Utc};
use gpui::{Context, Task};
use knotq_mcp::protocol;
use knotq_mcp::{call_tool, Outcome, ToolContext};
use knotq_storage_json::{clear_mcp_endpoint, save_mcp_endpoint, McpEndpoint};
use serde_json::Value;

mod http;
mod server;

use crate::app::KnotQApp;

/// A fresh bearer token.
///
/// Alphanumeric so it survives being pasted into a shell, a JSON config or an
/// environment variable without quoting, and drawn from the OS entropy source
/// `rand::thread_rng` seeds from — never a workspace value, which would make
/// the token predictable from data an attacker may already have.
fn random_token(len: usize) -> String {
    use rand::distributions::{Alphanumeric, DistString};
    Alphanumeric.sample_string(&mut rand::thread_rng(), len)
}

/// Length of the bearer token, in alphanumeric characters. 40 characters of
/// base62 is ~238 bits — far past anything guessable, and short enough to paste.
const TOKEN_LENGTH: usize = 40;

/// How long a single tool call may hold the main thread before it is worth
/// saying so. One frame at 60 Hz is 16 ms; a read that expands a year of
/// recurrences can legitimately take longer than that, but if it routinely does
/// the user will feel it as a stutter while an agent is working.
const MAIN_THREAD_BUDGET_MS: f64 = 16.0;

/// What an applied command turned out to have done.
struct Applied {
    /// False when the command was filtered out on its way through and nothing
    /// actually moved.
    changed: bool,
    /// For a tool that creates something, the field name and id to report.
    created: Option<(&'static str, String)>,
}

/// One tool call, in flight from a connection thread to the main thread.
pub(crate) struct McpJob {
    pub id: Value,
    pub name: String,
    pub arguments: Option<Value>,
    /// Bounded to one, so the connection thread that is already blocked on the
    /// reply cannot be outrun.
    pub reply: std::sync::mpsc::SyncSender<Value>,
}

pub(crate) type McpJobSender = Sender<McpJob>;

/// A running server. Dropping it stops the listener.
pub(crate) struct McpServer {
    pub port: u16,
    token: String,
    shutdown: Arc<AtomicBool>,
    /// Kept alive for the life of the server; the GPUI task ends when it is dropped.
    _task: Task<()>,
}

impl Drop for McpServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        // Unblock the listener's `accept` so the thread notices the flag and
        // exits, rather than lingering until the next connection.
        let _ = std::net::TcpStream::connect(("127.0.0.1", self.port));
        clear_mcp_endpoint();
    }
}

impl McpServer {
    /// Keep the descriptor honest when the setting changes without a restart.
    /// Tool evaluation reads the live setting on the main thread, but clients
    /// also use this field to explain their current access level before making
    /// a call.
    pub(crate) fn set_read_only(&self, read_only: bool) -> anyhow::Result<()> {
        save_mcp_endpoint(&McpEndpoint {
            url: format!("http://127.0.0.1:{}{}", self.port, http::MCP_PATH),
            port: self.port,
            token: self.token.clone(),
            read_only,
            protocol_version: protocol::PROTOCOL_VERSION.to_string(),
        })
    }
}

/// The stdio bridge shipped beside the desktop executable.
///
/// Keeping this resolution relative to the app rather than relying on `PATH`
/// means a client config remains valid after the user installs or updates
/// KnotQ, without a separate package manager or runtime.
pub(crate) fn bundled_bridge_path() -> Option<std::path::PathBuf> {
    let executable = if cfg!(windows) {
        "knotq-mcp.exe"
    } else {
        "knotq-mcp"
    };
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.join(executable)))
}

/// Start the server if the user has enabled it.
///
/// Returns `None` when the feature is off, which is the default. When it is on
/// the server runs for the whole life of the app: a client's saved
/// configuration should keep working across restarts without the user having to
/// re-arm anything.
pub(crate) fn start_if_enabled(
    settings: &knotq_model::AppSettings,
    cx: &mut Context<KnotQApp>,
) -> Option<McpServer> {
    if !settings.mcp.enabled {
        // Any descriptor left by a previous run points at a port nothing is
        // listening on now.
        clear_mcp_endpoint();
        return None;
    }
    match start(settings.mcp.port, settings.mcp.read_only, cx) {
        Ok(server) => Some(server),
        Err(e) => {
            eprintln!("[mcp] could not start the MCP server: {e}");
            clear_mcp_endpoint();
            None
        }
    }
}

pub(crate) fn start(
    preferred_port: u16,
    read_only: bool,
    cx: &mut Context<KnotQApp>,
) -> anyhow::Result<McpServer> {
    let listening = server::bind(preferred_port)?;
    let port = listening.port;
    // A fresh token every start. The endpoint file is rewritten alongside it, so
    // a client that reads the file each time keeps working, and a token that
    // leaked from a previous session stops working the moment the app restarts.
    let token = random_token(TOKEN_LENGTH);

    save_mcp_endpoint(&McpEndpoint {
        url: format!("http://127.0.0.1:{port}{}", http::MCP_PATH),
        port,
        token: token.clone(),
        read_only,
        protocol_version: protocol::PROTOCOL_VERSION.to_string(),
    })?;

    let (jobs_tx, jobs_rx) = async_channel::bounded::<McpJob>(32);
    let shutdown = Arc::new(AtomicBool::new(false));

    let listener_shutdown = Arc::clone(&shutdown);
    let listener_token = Arc::new(token.clone());
    std::thread::Builder::new()
        .name("knotq-mcp".into())
        .spawn(move || {
            server::serve(
                listening.listener,
                listener_token,
                jobs_tx,
                listener_shutdown,
            )
        })?;

    eprintln!(
        "[mcp] listening on http://127.0.0.1:{port}{}",
        http::MCP_PATH
    );
    Ok(McpServer {
        port,
        token,
        shutdown,
        _task: spawn_job_task(jobs_rx, cx),
    })
}

/// Drain tool calls on the main thread, one at a time.
fn spawn_job_task(jobs: Receiver<McpJob>, cx: &mut Context<KnotQApp>) -> Task<()> {
    cx.spawn(async move |weak: gpui::WeakEntity<KnotQApp>, cx| {
        while let Ok(job) = jobs.recv().await {
            let id = job.id.clone();
            // `cx.spawn` resumes on the foreground executor, so this closure
            // runs on the GPUI main thread — the same thread that applies a
            // keystroke. That is what serialises agent edits against the user's.
            let response = weak
                .update(cx, |app, cx| app.evaluate_mcp_job(&job, cx))
                .unwrap_or_else(|_| protocol::internal_error(id, "KnotQ is shutting down"));
            // The connection thread may already have timed out and gone; that
            // is not an error worth reporting.
            let _ = job.reply.send(response);
        }
    })
}

impl KnotQApp {
    /// Evaluate one tool call and, if it produced a command, apply it.
    ///
    /// Runs on the main thread. Everything it does is synchronous on purpose:
    /// the workspace must not change between the read that built the command
    /// and the apply that commits it, and the only way to guarantee that
    /// without a lock is not to yield in between.
    fn evaluate_mcp_job(&mut self, job: &McpJob, cx: &mut Context<Self>) -> Value {
        let started = Instant::now();
        let read_only = self.settings.mcp.read_only;

        // Read out of settings *before* taking the index: `indexed()` borrows
        // the state mutably, and settings live behind the same deref.
        let time_format = self.settings.time_format;
        let now = Utc::now();
        let today = Local::now().date_naive();

        // The index is built from the *store's* copy of the workspace, which
        // lags the live one whenever something has mutated `state.workspace`
        // directly. Flush that through first, exactly as the apply path itself
        // does, or a tool would read a stale workspace and — worse — build a
        // command against it (a `position` computed from an out-of-date line
        // count) that is then applied to the current one. Measured at ~0.2 ms.
        self.state.sync_store_from_workspace();

        let outcome = {
            let indexed = self.state.indexed();
            let ctx = ToolContext::new(indexed, now, today, time_format, read_only);
            call_tool(&job.name, job.arguments.as_ref(), &ctx)
        };

        let response = match outcome {
            Ok(Outcome::Write {
                command,
                mut response,
            }) => {
                match self.apply_from_agent(command, cx) {
                    // `changed` comes from whether a receipt came back, not from
                    // the fact that a command was built: a command can still be
                    // filtered out on its way through (a recurrence toggle that
                    // resolves to nothing), and claiming otherwise would tell the
                    // agent it had made a change it had not.
                    Ok(Applied { changed, created }) => {
                        // `create_scheme` / `create_folder` cannot name what
                        // they made until the apply returns, so splice the id in
                        // here rather than making the agent re-list and guess.
                        if let (Some((field, id)), Some(map)) = (created, response.as_object_mut())
                        {
                            map.insert(field.to_string(), Value::String(id));
                        }
                        protocol::applied_write_response(job.id.clone(), &response, changed)
                    }
                    // The command was legal when it was built and refused when
                    // it was applied. Report it as a refusal the model can read,
                    // not a transport failure.
                    Err(err) => protocol::tool_call_response(
                        job.id.clone(),
                        Err(knotq_mcp::ToolError::refused(err)),
                    ),
                }
            }
            other => protocol::tool_call_response(job.id.clone(), other),
        };

        let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
        if elapsed_ms > MAIN_THREAD_BUDGET_MS {
            eprintln!(
                "[mcp] `{}` held the main thread for {elapsed_ms:.1} ms (budget {MAIN_THREAD_BUDGET_MS:.0} ms)",
                job.name
            );
        }
        response
    }

    /// Apply an agent's command through the ordinary user path.
    ///
    /// Deliberately reuses `apply_result` rather than reaching into the store:
    /// the agent gets the same invariant checks, the same undo entry, and the
    /// same service signals as a typed edit, so a user can undo what an
    /// assistant did with the same keystroke they undo their own work with.
    fn apply_from_agent(
        &mut self,
        command: knotq_commands::Command,
        cx: &mut Context<Self>,
    ) -> Result<Applied, String> {
        self.apply_result(command, cx)
            .map(|receipt| Applied {
                changed: receipt.is_some(),
                created: receipt
                    .as_ref()
                    .and_then(|r| knotq_mcp::created_id(&r.inverse)),
            })
            .map_err(|err| err.to_string())
    }
}
