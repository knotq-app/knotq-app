use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::time::Duration as StdDuration;

use anyhow::{anyhow, Context as AnyhowContext, Result};
use async_channel::Receiver;
use chrono::Utc;
use futures::{pin_mut, select, FutureExt};
use gpui::{Context, Task};
use knotq_model::{
    DocumentId, ImageAssetFormat, ItemMedia, ReplicaId, SyncAccountSettings, SyncDocumentKind,
    Workspace, WorkspaceId,
};
use knotq_storage_json::{
    image_asset_path, load_local_sync_state, save_local_sync_state, save_workspace, workspace_path,
};
use knotq_sync::{
    DocumentResponse, ErrorResponse, LocalSyncState, NotificationScheduleSnapshot, PendingCrdtEdit,
    PullUpdatesResponse, PushUpdatesRequest, PushUpdatesResponse, StoredCrdtSnapshot,
    StoredCrdtUpdate, UpsertDocumentRequest, WorkspaceCrdtDocuments, MAX_SYNC_MEDIA_BYTES,
};
use sha2::{Digest, Sha256};

use super::{KnotQApp, NoticeModal, SyncRunStatus};

const SYNC_DEBOUNCE: StdDuration = StdDuration::from_secs(2);
const SYNC_POLL_INTERVAL: StdDuration = StdDuration::from_secs(30);
const SYNC_BATCH_LIMIT: usize = 50;

#[derive(Clone)]
struct SyncSnapshot {
    workspace: Workspace,
    account: SyncAccountSettings,
    replica_id: ReplicaId,
    pending: Vec<PendingCrdtEdit>,
    notification_schedule: NotificationScheduleSnapshot,
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
    remaining_pending: usize,
    forced_snapshot_applied: bool,
}

#[derive(Clone, Copy)]
struct SyncDocumentRef {
    document: DocumentId,
    kind: SyncDocumentKind,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct SyncMediaAsset {
    document: DocumentId,
    asset: uuid::Uuid,
    format: ImageAssetFormat,
}

impl SyncMediaAsset {
    fn image_name(self) -> String {
        format!("{}.{}", self.asset, self.format.extension())
    }
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

impl KnotQApp {
    fn show_sync_snapshot_notice(&mut self, cx: &mut Context<Self>) {
        self.notice_modal = Some(NoticeModal {
            title: "Sync snapshot applied".to_string(),
            message: "This device was far enough behind that the sync server had already compacted older CRDT changes. KnotQ applied the latest compacted snapshot and then continued syncing from there.".to_string(),
            button_label: "OK".to_string(),
        });
        cx.notify();
    }
}

async fn run_sync_once(weak: &gpui::WeakEntity<KnotQApp>, cx: &mut gpui::AsyncApp) {
    let snapshot = weak
        .update(cx, |app, _cx| {
            if app.workspace_save_blocked_reason.is_some() {
                return None;
            }
            let account = app.settings.sync_account.clone()?;
            if !account.supports_sync {
                app.sync_run_status = SyncRunStatus::Idle;
                _cx.notify();
                return None;
            }
            app.state.sync_store_from_compat();
            let pending = app.state.pending_crdt_edits();
            app.sync_run_status = SyncRunStatus::Running {
                pending: pending.len(),
            };
            _cx.notify();
            let notification_schedule = crate::notifications::notification_schedule_snapshot(
                &app.workspace,
                app.settings.notification_defaults,
                Utc::now(),
                0,
            );
            Some(SyncSnapshot {
                workspace: app.workspace.clone(),
                account,
                replica_id: app.settings.replica_id,
                pending,
                notification_schedule,
            })
        })
        .ok()
        .flatten();

    let Some(snapshot) = snapshot else {
        return;
    };

    let result = cx
        .background_executor()
        .spawn(async move { sync_snapshot(snapshot) })
        .await;

    match result {
        Ok(result) => {
            let remote_updates_applied = result.remote_updates_applied;
            let pushed = result.pushed.clone();
            let workspace = result.workspace.clone();
            let remaining_pending = result.remaining_pending;
            let forced_snapshot_applied = result.forced_snapshot_applied;
            let _ = weak.update(cx, |app, cx| {
                for pushed in pushed {
                    app.state
                        .clear_pushed_crdt_edits(pushed.document, pushed.through_local_sequence);
                }
                app.sync_run_status = SyncRunStatus::Synced {
                    pending: remaining_pending,
                };
                if remote_updates_applied > 0 {
                    app.state.replace_workspace_from_sync(workspace);
                    app.service_bus.signal_save();
                    app.service_bus.signal_notifications();
                    app.service_bus.signal_timeline();
                }
                if forced_snapshot_applied {
                    app.show_sync_snapshot_notice(cx);
                }
                cx.notify();
            });
        }
        Err(err) => {
            eprintln!("sync failed: {err:#}");
            let message = format!("{err:#}");
            let _ = weak.update(cx, |app, cx| {
                app.sync_run_status = SyncRunStatus::Error {
                    message,
                    pending: app.state.pending_crdt_edits().len(),
                };
                cx.notify();
            });
        }
    }
}

fn sync_snapshot(snapshot: SyncSnapshot) -> Result<SyncRunResult> {
    let path = workspace_path();
    let mut workspace = snapshot.workspace;
    workspace.ensure_sync_metadata();
    let server_workspace_id = snapshot.account.workspace_id.unwrap_or(workspace.id);

    let mut local_state = load_local_sync_state(&path).unwrap_or_default();
    configure_local_state(
        &mut local_state,
        server_workspace_id,
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
    let mut forced_snapshot_applied = false;

    upsert_documents(&client, server_workspace_id, sync_documents(&workspace))?;
    upload_local_media_assets(&client, &mut local_state, server_workspace_id, &workspace)?;

    let workspace_doc = SyncDocumentRef {
        document: workspace.sync.id,
        kind: SyncDocumentKind::PersonalWorkspace,
    };
    let workspace_pull = pull_document_all(
        &client,
        &local_state,
        server_workspace_id,
        workspace_doc,
        snapshot.replica_id,
    )?;
    remote_latest.insert(workspace_doc.document, workspace_pull.latest_sequence);
    let workspace_updates = workspace_pull.updates;
    forced_snapshot_applied |= workspace_pull.forced_snapshot;
    if !workspace_updates.is_empty() {
        let outcome = crdt_docs.apply_remote_updates(&workspace, &workspace_updates);
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

    upsert_documents(&client, server_workspace_id, sync_documents(&workspace))?;
    download_missing_media_assets(&client, server_workspace_id, &workspace)?;

    let mut scheme_updates = Vec::new();
    for doc in scheme_documents(&workspace) {
        let pull = pull_document_all(
            &client,
            &local_state,
            server_workspace_id,
            doc,
            snapshot.replica_id,
        )?;
        remote_latest.insert(doc.document, pull.latest_sequence);
        forced_snapshot_applied |= pull.forced_snapshot;
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
    download_missing_media_assets(&client, server_workspace_id, &workspace)?;

    queue_bootstrap_updates(&mut local_state, &workspace, &remote_latest);
    push_pending_documents(
        &client,
        &mut local_state,
        server_workspace_id,
        &mut pushed,
        &snapshot.notification_schedule,
    )?;

    save_local_sync_state(&path, &local_state)?;
    if remote_updates_applied > 0 {
        save_workspace(&path, &workspace)?;
    }

    Ok(SyncRunResult {
        workspace,
        pushed,
        remote_updates_applied,
        remaining_pending: local_state.pending.len(),
        forced_snapshot_applied,
    })
}

fn pull_response_updates(response: &PullUpdatesResponse) -> Vec<StoredCrdtUpdate> {
    let mut updates = Vec::new();
    if let Some(snapshot) = &response.snapshot {
        updates.push(snapshot_as_update(snapshot));
    }
    updates.extend(response.updates.iter().cloned());
    updates
}

fn snapshot_as_update(snapshot: &StoredCrdtSnapshot) -> StoredCrdtUpdate {
    StoredCrdtUpdate {
        workspace_id: snapshot.workspace_id,
        document: snapshot.document,
        kind: snapshot.kind,
        replica_id: ReplicaId::new(),
        sequence: snapshot.sequence,
        received_at: snapshot.compacted_at,
        update_v1: snapshot.update_v1.clone(),
    }
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

struct AccumulatedPull {
    updates: Vec<StoredCrdtUpdate>,
    latest_sequence: u64,
    forced_snapshot: bool,
}

/// Pull a document one bounded page at a time, following the server's `has_more`
/// flag until caught up. Paging keeps individual responses small even when a
/// replica is far behind.
fn pull_document_all(
    client: &SyncHttpClient,
    local_state: &LocalSyncState,
    workspace_id: WorkspaceId,
    doc: SyncDocumentRef,
    replica_id: ReplicaId,
) -> Result<AccumulatedPull> {
    let mut after = local_state
        .document_cursors
        .get(&doc.document)
        .map(|cursor| cursor.last_pulled_sequence)
        .unwrap_or(0);
    let mut updates = Vec::new();
    let mut latest_sequence;
    let mut forced_snapshot = false;
    loop {
        let response = client.pull_updates(workspace_id, doc.document, after, replica_id)?;
        latest_sequence = response.latest_sequence;
        forced_snapshot |= response.forced_snapshot;
        let page = pull_response_updates(&response);
        let page_max = page.iter().map(|update| update.sequence).max();
        updates.extend(page);
        match page_max {
            // Advance only while the cursor strictly moves forward, so a
            // misbehaving server that keeps reporting `has_more` cannot wedge
            // the client in an infinite pull loop.
            Some(max) if response.has_more && max > after => after = max,
            _ => break,
        }
    }
    Ok(AccumulatedPull {
        updates,
        latest_sequence,
        forced_snapshot,
    })
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
    notification_schedule: &NotificationScheduleSnapshot,
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
        let mut request = local_state
            .next_push_request(document, SYNC_BATCH_LIMIT)
            .ok_or_else(|| anyhow!("missing push request for pending document"))?;
        let through_local_sequence = pending
            .iter()
            .map(|edit| edit.local_sequence)
            .max()
            .unwrap_or(0);
        let mut notification_schedule = notification_schedule.clone();
        notification_schedule.sequence = through_local_sequence;
        request.notification_schedule = Some(notification_schedule);
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

fn workspace_media_assets(workspace: &Workspace) -> Vec<SyncMediaAsset> {
    let mut seen = HashSet::new();
    let mut assets = Vec::new();
    for scheme in workspace.iter_schemes() {
        let Some(meta) = workspace.scheme_sync.get(&scheme.id) else {
            continue;
        };
        for item in &scheme.items {
            for media in &item.media {
                let ItemMedia::Image { asset, format, .. } = media;
                let media = SyncMediaAsset {
                    document: meta.id,
                    asset: *asset,
                    format: *format,
                };
                if seen.insert(media) {
                    assets.push(media);
                }
            }
        }
    }
    assets
}

fn upload_local_media_assets(
    client: &SyncHttpClient,
    local_state: &mut LocalSyncState,
    workspace_id: WorkspaceId,
    workspace: &Workspace,
) -> Result<()> {
    for media in workspace_media_assets(workspace) {
        let path = image_asset_path(media.asset, media.format.extension());
        let Ok(metadata) = fs::metadata(&path) else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        let byte_length = metadata.len();
        if byte_length > MAX_SYNC_MEDIA_BYTES as u64 {
            return Err(anyhow!(
                "image {} is {} bytes, above the {} byte sync limit",
                media.image_name(),
                byte_length,
                MAX_SYNC_MEDIA_BYTES
            ));
        }
        let image_name = media.image_name();
        let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        if bytes.len() > MAX_SYNC_MEDIA_BYTES {
            return Err(anyhow!(
                "image {} is {} bytes, above the {} byte sync limit",
                image_name,
                bytes.len(),
                MAX_SYNC_MEDIA_BYTES
            ));
        }
        let sha256 = media_sha256(&bytes);
        if local_state.media_upload_is_current(&image_name, media.document, byte_length, &sha256) {
            continue;
        }
        client.upload_media_asset(workspace_id, media, &bytes)?;
        local_state.mark_media_uploaded(image_name, media.document, byte_length, sha256);
    }
    Ok(())
}

fn download_missing_media_assets(
    client: &SyncHttpClient,
    workspace_id: WorkspaceId,
    workspace: &Workspace,
) -> Result<()> {
    for media in workspace_media_assets(workspace) {
        let path = image_asset_path(media.asset, media.format.extension());
        if path.exists() {
            continue;
        }
        let bytes = client.download_media_asset(workspace_id, media)?;
        if bytes.len() > MAX_SYNC_MEDIA_BYTES {
            return Err(anyhow!(
                "downloaded image {} is {} bytes, above the {} byte sync limit",
                media.image_name(),
                bytes.len(),
                MAX_SYNC_MEDIA_BYTES
            ));
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        fs::write(&path, bytes).with_context(|| format!("write {}", path.display()))?;
    }
    Ok(())
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

    fn upload_media_asset(
        &self,
        workspace_id: WorkspaceId,
        media: SyncMediaAsset,
        bytes: &[u8],
    ) -> Result<()> {
        let url = self.media_url(workspace_id, media);
        self.authorized(ureq::put(&url))
            .set("content-type", media_content_type(media.format))
            .send_bytes(bytes)
            .map_err(sync_http_error)?;
        Ok(())
    }

    fn download_media_asset(
        &self,
        workspace_id: WorkspaceId,
        media: SyncMediaAsset,
    ) -> Result<Vec<u8>> {
        let url = self.media_url(workspace_id, media);
        let response = self
            .authorized(ureq::get(&url))
            .call()
            .map_err(sync_http_error)?;
        let mut reader = response
            .into_reader()
            .take((MAX_SYNC_MEDIA_BYTES + 1) as u64);
        let mut bytes = Vec::new();
        reader
            .read_to_end(&mut bytes)
            .with_context(|| format!("read media response from {url}"))?;
        if bytes.len() > MAX_SYNC_MEDIA_BYTES {
            return Err(anyhow!(
                "sync backend returned image {} above the {} byte sync limit",
                media.image_name(),
                MAX_SYNC_MEDIA_BYTES
            ));
        }
        Ok(bytes)
    }

    fn media_url(&self, workspace_id: WorkspaceId, media: SyncMediaAsset) -> String {
        format!(
            "{}/v1/workspaces/{}/documents/{}/media/{}",
            self.api_base,
            workspace_id,
            media.document,
            media.image_name()
        )
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

fn media_content_type(format: ImageAssetFormat) -> &'static str {
    match format {
        ImageAssetFormat::Png => "image/png",
        ImageAssetFormat::Jpeg => "image/jpeg",
        ImageAssetFormat::Webp => "image/webp",
        ImageAssetFormat::Gif => "image/gif",
        ImageAssetFormat::Svg => "image/svg+xml",
        ImageAssetFormat::Bmp => "image/bmp",
        ImageAssetFormat::Tiff => "image/tiff",
    }
}

fn media_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
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
    let normalized = match trimmed {
        "http://127.0.0.1:7878" | "http://localhost:7878" => "http://127.0.0.1:8787".to_string(),
        _ => trimmed.to_string(),
    };
    // The bearer token and all workspace contents travel over this URL. Refuse
    // plaintext HTTP to anything other than a loopback dev server so a misconfig
    // (or tampered settings file) can't silently leak credentials in the clear.
    if !is_secure_api_base(&normalized) {
        return Err(anyhow!("sync API URL must use https:// (got {normalized})"));
    }
    Ok(normalized)
}

fn is_secure_api_base(url: &str) -> bool {
    if let Some(host) = url.strip_prefix("https://") {
        return !host.is_empty();
    }
    if let Some(rest) = url.strip_prefix("http://") {
        let host = rest
            .split(['/', ':'])
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        return matches!(host.as_str(), "127.0.0.1" | "localhost" | "[::1]" | "::1");
    }
    false
}

#[cfg(test)]
mod tests {
    use super::normalize_api_base;

    #[test]
    fn https_urls_are_accepted_and_trimmed() {
        assert_eq!(
            normalize_api_base("https://sync.example.com/").unwrap(),
            "https://sync.example.com"
        );
    }

    #[test]
    fn loopback_http_is_allowed_for_dev() {
        assert_eq!(
            normalize_api_base("http://localhost:7878").unwrap(),
            "http://127.0.0.1:8787"
        );
        assert!(normalize_api_base("http://127.0.0.1:8787").is_ok());
    }

    #[test]
    fn plaintext_http_to_remote_hosts_is_rejected() {
        assert!(normalize_api_base("http://sync.example.com").is_err());
        assert!(normalize_api_base("ftp://sync.example.com").is_err());
        assert!(normalize_api_base("").is_err());
    }
}
