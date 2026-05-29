use std::collections::{HashMap, HashSet};
use std::time::Duration as StdDuration;

use anyhow::{anyhow, Context as AnyhowContext, Result};
use async_channel::Receiver;
use chrono::Utc;
use futures::{pin_mut, select, FutureExt};
use gpui::{Context, Task};
use knotq_model::{
    DocumentId, ReplicaId, SyncAccountSettings, SyncDocumentKind, Workspace, WorkspaceId,
};
use knotq_storage_json::{
    load_local_sync_state, save_local_sync_state, save_workspace, workspace_path,
};
use knotq_sync::{
    DocumentResponse, ErrorResponse, LocalSyncState, PendingCrdtEdit, PullUpdatesResponse,
    PushUpdatesRequest, PushUpdatesResponse, UpsertDocumentRequest, WorkspaceCrdtDocuments,
};

use super::KnotQApp;

const SYNC_DEBOUNCE: StdDuration = StdDuration::from_secs(2);
const SYNC_POLL_INTERVAL: StdDuration = StdDuration::from_secs(30);
const SYNC_BATCH_LIMIT: usize = 50;

#[derive(Clone)]
struct SyncSnapshot {
    workspace: Workspace,
    account: SyncAccountSettings,
    replica_id: ReplicaId,
    pending: Vec<PendingCrdtEdit>,
}

#[derive(Clone, Copy, Debug)]
struct PushedDocument {
    document: DocumentId,
    through_local_sequence: u64,
}

struct SyncRunResult {
    workspace: Workspace,
    pushed: Vec<PushedDocument>,
    remote_updates_applied: usize,
}

#[derive(Clone, Copy)]
struct SyncDocumentRef {
    document: DocumentId,
    kind: SyncDocumentKind,
}

struct SyncHttpClient {
    api_base: String,
    bearer_token: String,
}

pub(crate) fn spawn_sync_task(sync_rx: Receiver<()>, cx: &mut Context<KnotQApp>) -> Task<()> {
    cx.spawn(
        async move |weak: gpui::WeakEntity<KnotQApp>, cx: &mut gpui::AsyncApp| {
            run_sync_once(&weak, cx).await;
            loop {
                let timer = cx.background_executor().timer(SYNC_POLL_INTERVAL).fuse();
                let signal = sync_rx.recv().fuse();
                pin_mut!(timer, signal);
                let mut signaled = false;
                select! {
                    _ = timer => {}
                    result = signal => {
                        if result.is_err() {
                            break;
                        }
                        signaled = true;
                    }
                }
                if signaled {
                    cx.background_executor().timer(SYNC_DEBOUNCE).await;
                    while sync_rx.try_recv().is_ok() {}
                }
                run_sync_once(&weak, cx).await;
            }
        },
    )
}

async fn run_sync_once(weak: &gpui::WeakEntity<KnotQApp>, cx: &mut gpui::AsyncApp) {
    let snapshot = weak
        .update(cx, |app, _cx| {
            if app.workspace_save_blocked_reason.is_some() {
                return None;
            }
            let account = app.settings.sync_account.clone()?;
            app.state.sync_store_from_compat();
            Some(SyncSnapshot {
                workspace: app.workspace.clone(),
                account,
                replica_id: app.settings.replica_id,
                pending: app.state.pending_crdt_edits(),
            })
        })
        .ok()
        .flatten();

    let Some(snapshot) = snapshot else {
        return;
    };
    if !snapshot.account.supports_sync {
        return;
    }

    let result = cx
        .background_executor()
        .spawn(async move { sync_snapshot(snapshot) })
        .await;

    match result {
        Ok(result) => {
            let remote_updates_applied = result.remote_updates_applied;
            let pushed = result.pushed.clone();
            let workspace = result.workspace.clone();
            let _ = weak.update(cx, |app, cx| {
                for pushed in pushed {
                    app.state
                        .clear_pushed_crdt_edits(pushed.document, pushed.through_local_sequence);
                }
                if remote_updates_applied > 0 {
                    app.state.replace_workspace_from_sync(workspace);
                    app.service_bus.signal_save();
                    app.service_bus.signal_notifications();
                    app.service_bus.signal_timeline();
                }
                cx.notify();
            });
        }
        Err(err) => {
            eprintln!("sync failed: {err:#}");
        }
    }
}

fn sync_snapshot(snapshot: SyncSnapshot) -> Result<SyncRunResult> {
    let path = workspace_path();
    let mut workspace = snapshot.workspace;
    workspace.ensure_sync_metadata();

    let mut local_state = load_local_sync_state(&path).unwrap_or_default();
    configure_local_state(
        &mut local_state,
        workspace.id,
        snapshot.replica_id,
        &snapshot.account,
    );
    merge_pending(&mut local_state, snapshot.pending);

    let client = SyncHttpClient {
        api_base: normalize_api_base(&snapshot.account.api_base)?,
        bearer_token: snapshot.account.bearer_token.clone(),
    };
    let mut crdt_docs = WorkspaceCrdtDocuments::try_new(&workspace)?;
    let mut remote_latest = HashMap::new();
    let mut pushed = Vec::new();
    let mut remote_updates_applied = 0;

    upsert_documents(&client, workspace.id, sync_documents(&workspace))?;

    let workspace_doc = SyncDocumentRef {
        document: workspace.sync.id,
        kind: SyncDocumentKind::PersonalWorkspace,
    };
    let workspace_pull = pull_document(
        &client,
        &local_state,
        workspace.id,
        workspace_doc,
        snapshot.replica_id,
    )?;
    remote_latest.insert(workspace_doc.document, workspace_pull.latest_sequence);
    if !workspace_pull.updates.is_empty() {
        let outcome = crdt_docs.apply_remote_updates(&workspace, &workspace_pull.updates);
        if !outcome.is_ok() {
            return Err(anyhow!("workspace CRDT apply failed: {:?}", outcome.errors));
        }
        remote_updates_applied += outcome.applied;
        workspace = outcome.workspace;
    }
    local_state.mark_pulled(
        workspace_doc.document,
        workspace_doc.kind,
        workspace_pull.latest_sequence,
    );

    upsert_documents(&client, workspace.id, sync_documents(&workspace))?;

    let mut scheme_updates = Vec::new();
    for doc in scheme_documents(&workspace) {
        let pull = pull_document(
            &client,
            &local_state,
            workspace.id,
            doc,
            snapshot.replica_id,
        )?;
        remote_latest.insert(doc.document, pull.latest_sequence);
        if !pull.updates.is_empty() {
            scheme_updates.extend(pull.updates);
        }
        local_state.mark_pulled(doc.document, doc.kind, pull.latest_sequence);
    }
    if !scheme_updates.is_empty() {
        let outcome = crdt_docs.apply_remote_updates(&workspace, &scheme_updates);
        if !outcome.is_ok() {
            return Err(anyhow!("scheme CRDT apply failed: {:?}", outcome.errors));
        }
        remote_updates_applied += outcome.applied;
        workspace = outcome.workspace;
    }

    queue_bootstrap_updates(&mut local_state, &workspace, &remote_latest);
    push_pending_documents(&client, &mut local_state, workspace.id, &mut pushed)?;

    save_local_sync_state(&path, &local_state)?;
    if remote_updates_applied > 0 {
        save_workspace(&path, &workspace)?;
    }

    Ok(SyncRunResult {
        workspace,
        pushed,
        remote_updates_applied,
    })
}

fn configure_local_state(
    local_state: &mut LocalSyncState,
    workspace_id: WorkspaceId,
    replica_id: ReplicaId,
    account: &SyncAccountSettings,
) {
    local_state.workspace_id = Some(workspace_id);
    local_state.replica_id = Some(replica_id);
    local_state.server_url = Some(account.api_base.clone());
    local_state.bearer_token = Some(account.bearer_token.clone());
}

fn merge_pending(local_state: &mut LocalSyncState, pending: Vec<PendingCrdtEdit>) {
    for edit in pending {
        if !local_state.pending.iter().any(|existing| {
            existing.operation_id == edit.operation_id
                && existing.document == edit.document
                && existing.local_sequence == edit.local_sequence
        }) {
            local_state.push_pending(edit);
        }
    }
}

fn upsert_documents(
    client: &SyncHttpClient,
    workspace_id: WorkspaceId,
    docs: Vec<SyncDocumentRef>,
) -> Result<()> {
    let mut seen = HashSet::new();
    for doc in docs {
        if seen.insert(doc.document) {
            client.upsert_document(workspace_id, doc)?;
        }
    }
    Ok(())
}

fn pull_document(
    client: &SyncHttpClient,
    local_state: &LocalSyncState,
    workspace_id: WorkspaceId,
    doc: SyncDocumentRef,
    replica_id: ReplicaId,
) -> Result<PullUpdatesResponse> {
    let after = local_state
        .document_cursors
        .get(&doc.document)
        .map(|cursor| cursor.last_pulled_sequence)
        .unwrap_or(0);
    client.pull_updates(workspace_id, doc.document, after, replica_id)
}

fn queue_bootstrap_updates(
    local_state: &mut LocalSyncState,
    workspace: &Workspace,
    remote_latest: &HashMap<DocumentId, u64>,
) {
    let mut next_sequence = local_state
        .pending
        .iter()
        .map(|edit| edit.local_sequence)
        .max()
        .unwrap_or(0)
        + 1;
    for update in WorkspaceCrdtDocuments::snapshot_updates(workspace).updates {
        if remote_latest.get(&update.document).copied().unwrap_or(0) != 0 {
            continue;
        }
        if local_state
            .pending
            .iter()
            .any(|pending| pending.document == update.document)
        {
            continue;
        }
        if local_state
            .document_cursors
            .get(&update.document)
            .is_some_and(|cursor| cursor.last_pushed_sequence > 0)
        {
            continue;
        }
        local_state.push_pending(PendingCrdtEdit {
            operation_id: knotq_model::OperationId::new(),
            workspace_id: workspace.id,
            replica_id: local_state.replica_id.unwrap_or_default(),
            local_sequence: next_sequence,
            created_at: Utc::now(),
            document: update.document,
            kind: update.kind,
            update_v1: update.update_v1,
        });
        next_sequence += 1;
    }
}

fn push_pending_documents(
    client: &SyncHttpClient,
    local_state: &mut LocalSyncState,
    workspace_id: WorkspaceId,
    pushed: &mut Vec<PushedDocument>,
) -> Result<()> {
    loop {
        let Some(document) = local_state.pending.front().map(|edit| edit.document) else {
            return Ok(());
        };
        let pending = local_state.pending_for_document(document, SYNC_BATCH_LIMIT);
        if pending.is_empty() {
            return Ok(());
        }
        let kind = pending[0].kind;
        client.upsert_document(workspace_id, SyncDocumentRef { document, kind })?;
        let request = local_state
            .next_push_request(document, SYNC_BATCH_LIMIT)
            .ok_or_else(|| anyhow!("missing push request for pending document"))?;
        let through_local_sequence = pending
            .iter()
            .map(|edit| edit.local_sequence)
            .max()
            .unwrap_or(0);
        let response = client.push_updates(workspace_id, document, &request)?;
        if response.accepted != request.updates.len() {
            return Err(anyhow!(
                "sync backend accepted {}/{} updates for {}",
                response.accepted,
                request.updates.len(),
                document
            ));
        }
        local_state.mark_pushed(document, through_local_sequence);
        pushed.push(PushedDocument {
            document,
            through_local_sequence,
        });
    }
}

fn sync_documents(workspace: &Workspace) -> Vec<SyncDocumentRef> {
    let mut docs = vec![SyncDocumentRef {
        document: workspace.sync.id,
        kind: SyncDocumentKind::PersonalWorkspace,
    }];
    docs.extend(scheme_documents(workspace));
    docs
}

fn scheme_documents(workspace: &Workspace) -> Vec<SyncDocumentRef> {
    workspace
        .scheme_sync
        .values()
        .filter(|meta| meta.kind == SyncDocumentKind::Scheme)
        .map(|meta| SyncDocumentRef {
            document: meta.id,
            kind: SyncDocumentKind::Scheme,
        })
        .collect()
}

impl SyncHttpClient {
    fn upsert_document(&self, workspace_id: WorkspaceId, doc: SyncDocumentRef) -> Result<()> {
        let url = format!(
            "{}/v1/workspaces/{}/documents/{}",
            self.api_base, workspace_id, doc.document
        );
        self.put_json::<_, DocumentResponse>(&url, &UpsertDocumentRequest { kind: doc.kind })
            .map(|_| ())
    }

    fn pull_updates(
        &self,
        workspace_id: WorkspaceId,
        document: DocumentId,
        after: u64,
        replica_id: ReplicaId,
    ) -> Result<PullUpdatesResponse> {
        let url = format!(
            "{}/v1/workspaces/{}/documents/{}/updates?after={}&exclude_replica={}",
            self.api_base, workspace_id, document, after, replica_id
        );
        self.get_json(&url)
    }

    fn push_updates(
        &self,
        workspace_id: WorkspaceId,
        document: DocumentId,
        request: &PushUpdatesRequest,
    ) -> Result<PushUpdatesResponse> {
        let url = format!(
            "{}/v1/workspaces/{}/documents/{}/updates",
            self.api_base, workspace_id, document
        );
        self.post_json(&url, request)
    }

    fn get_json<T: serde::de::DeserializeOwned>(&self, url: &str) -> Result<T> {
        self.authorized(ureq::get(url))
            .call()
            .map_err(sync_http_error)?
            .into_json()
            .with_context(|| format!("parse sync response from {url}"))
    }

    fn post_json<T, R>(&self, url: &str, body: &T) -> Result<R>
    where
        T: serde::Serialize,
        R: serde::de::DeserializeOwned,
    {
        self.authorized(ureq::post(url))
            .send_json(serde_json::to_value(body)?)
            .map_err(sync_http_error)?
            .into_json()
            .with_context(|| format!("parse sync response from {url}"))
    }

    fn put_json<T, R>(&self, url: &str, body: &T) -> Result<R>
    where
        T: serde::Serialize,
        R: serde::de::DeserializeOwned,
    {
        self.authorized(ureq::put(url))
            .send_json(serde_json::to_value(body)?)
            .map_err(sync_http_error)?
            .into_json()
            .with_context(|| format!("parse sync response from {url}"))
    }

    fn authorized(&self, request: ureq::Request) -> ureq::Request {
        request
            .timeout(SYNC_POLL_INTERVAL)
            .set("authorization", &format!("Bearer {}", self.bearer_token))
    }
}

fn sync_http_error(error: ureq::Error) -> anyhow::Error {
    match error {
        ureq::Error::Status(status, response) => {
            let code = response
                .into_json::<ErrorResponse>()
                .map(|error| error.code)
                .unwrap_or_else(|_| status.to_string());
            anyhow!("sync backend rejected request: {code}")
        }
        error => anyhow!("sync backend request failed: {error}"),
    }
}

fn normalize_api_base(raw: &str) -> Result<String> {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err(anyhow!("sync API URL is empty"));
    }
    Ok(match trimmed {
        "http://127.0.0.1:7878" | "http://localhost:7878" => "http://127.0.0.1:8787".to_string(),
        _ => trimmed.to_string(),
    })
}
