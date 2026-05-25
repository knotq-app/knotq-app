use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use chrono::{DateTime, Utc};
use knotq_model::{DocumentId, ReplicaId, ShareId, SyncDocumentKind, WorkspaceId};
use knotq_sync::{
    AccessRole, AckNotificationScheduleRequest, AckNotificationScheduleResponse, AuthSession,
    BackgroundPushIntent, BackgroundPushKind, CrdtDocumentUpdate, DevLoginRequest,
    DeviceNotificationScheduleStatus, DevicePlatform, DocumentResponse, ErrorResponse,
    MarkNotificationScheduleChangedRequest, MarkNotificationScheduleChangedResponse,
    NotificationPermissionState, NotificationScheduleChangeReason, NotificationScheduleDeviceState,
    NotificationScheduleStatusResponse, PullUpdatesResponse, PushChannel, PushEnvironment,
    PushUpdatesRequest, PushUpdatesResponse, RegisterDeviceRequest, RegisterDeviceResponse,
    ShareDocumentRequest, ShareDocumentResponse, StoredCrdtUpdate, UserId, SYNC_API_VERSION,
};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone)]
pub struct AppState {
    inner: Arc<RwLock<BackendStore>>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            inner: Arc::new(RwLock::new(BackendStore::default())),
        }
    }
}

#[derive(Default)]
struct BackendStore {
    users_by_email: HashMap<String, UserId>,
    sessions: HashMap<String, UserId>,
    workspace_owners: HashMap<WorkspaceId, UserId>,
    devices: HashMap<DeviceKey, DeviceRecord>,
    notification_schedules: HashMap<WorkspaceId, WorkspaceNotificationSchedule>,
    background_push_outbox: Vec<BackgroundPushIntent>,
    next_background_push_id: u64,
    documents: HashMap<DocumentKey, DocumentRecord>,
    updates: HashMap<DocumentKey, Vec<StoredCrdtUpdate>>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct DeviceKey {
    workspace_id: WorkspaceId,
    user_id: UserId,
    replica_id: ReplicaId,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct DocumentKey {
    workspace_id: WorkspaceId,
    document: DocumentId,
}

struct DeviceRecord {
    display_name: Option<String>,
    platform: DevicePlatform,
    app_version: Option<String>,
    push_channel: Option<PushChannel>,
    push_token: Option<String>,
    push_environment: Option<PushEnvironment>,
    notification_permission: NotificationPermissionState,
    local_scheduler_supported: Option<bool>,
    acknowledged_schedule_revision: u64,
    last_ack_at: Option<DateTime<Utc>>,
    scheduled_until: Option<DateTime<Utc>>,
    scheduled_occurrence_ids: Vec<String>,
    last_background_push_enqueued_at: Option<DateTime<Utc>>,
    registered_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Default)]
struct WorkspaceNotificationSchedule {
    revision: u64,
    changed_at: Option<DateTime<Utc>>,
}

struct DocumentRecord {
    kind: SyncDocumentKind,
    owner: UserId,
    grants: HashMap<UserId, AccessRole>,
    shares: HashMap<ShareId, ShareGrant>,
    next_sequence: u64,
}

struct ShareGrant {
    grantee: UserId,
    role: AccessRole,
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    api_version: &'static str,
}

#[derive(Deserialize)]
struct PullUpdatesQuery {
    #[serde(default)]
    after: u64,
    #[serde(default)]
    exclude_replica: Option<ReplicaId>,
}

pub fn app() -> Router {
    app_with_state(AppState::default())
}

pub fn app_with_state(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/v1/auth/dev-login", post(dev_login))
        .route(
            "/v1/workspaces/{workspace_id}/devices",
            post(register_device),
        )
        .route(
            "/v1/workspaces/{workspace_id}/notification-schedule",
            get(notification_schedule_status),
        )
        .route(
            "/v1/workspaces/{workspace_id}/notification-schedule/changes",
            post(mark_notification_schedule_changed),
        )
        .route(
            "/v1/workspaces/{workspace_id}/notification-schedule/ack",
            post(ack_notification_schedule),
        )
        .route(
            "/v1/workspaces/{workspace_id}/documents/{document_id}",
            put(upsert_document),
        )
        .route(
            "/v1/workspaces/{workspace_id}/documents/{document_id}/updates",
            post(push_updates).get(pull_updates),
        )
        .route(
            "/v1/workspaces/{workspace_id}/documents/{document_id}/shares",
            post(share_document),
        )
        .with_state(state)
}

async fn healthz() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        api_version: SYNC_API_VERSION,
    })
}

async fn dev_login(
    State(state): State<AppState>,
    Json(request): Json<DevLoginRequest>,
) -> Result<Json<AuthSession>, ApiError> {
    let email = normalize_email(&request.email)?;
    let user_id = {
        let mut store = write_store(&state)?;
        *store
            .users_by_email
            .entry(email.clone())
            .or_insert_with(UserId::new)
    };
    let token = make_session_token(&email, user_id);
    write_store(&state)?.sessions.insert(token.clone(), user_id);
    Ok(Json(AuthSession {
        user_id,
        bearer_token: token,
    }))
}

async fn register_device(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(workspace_id): Path<WorkspaceId>,
    Json(request): Json<RegisterDeviceRequest>,
) -> Result<Json<RegisterDeviceResponse>, ApiError> {
    validate_push_registration(&request)?;
    let user_id = authenticate(&state, &headers)?;
    let mut store = write_store(&state)?;
    ensure_workspace_owner(&mut store, workspace_id, user_id)?;
    let schedule_revision = current_notification_schedule_revision_mut(&mut store, workspace_id);
    let now = Utc::now();
    let key = DeviceKey {
        workspace_id,
        user_id,
        replica_id: request.replica_id,
    };
    let display_name = clean_optional_string(request.display_name);
    let app_version = clean_optional_string(request.app_version);
    let push_token = clean_optional_string(request.push_token);

    store
        .devices
        .entry(key)
        .and_modify(|device| {
            device.display_name = display_name.clone();
            device.platform = request.platform;
            device.app_version = app_version.clone();
            device.push_channel = request.push_channel;
            device.push_token = push_token.clone();
            device.push_environment = request.push_environment;
            device.notification_permission = request.notification_permission;
            device.local_scheduler_supported = request.local_scheduler_supported;
            device.updated_at = now;
        })
        .or_insert_with(|| DeviceRecord {
            display_name,
            platform: request.platform,
            app_version,
            push_channel: request.push_channel,
            push_token,
            push_environment: request.push_environment,
            notification_permission: request.notification_permission,
            local_scheduler_supported: request.local_scheduler_supported,
            acknowledged_schedule_revision: 0,
            last_ack_at: None,
            scheduled_until: None,
            scheduled_occurrence_ids: Vec::new(),
            last_background_push_enqueued_at: None,
            registered_at: now,
            updated_at: now,
        });
    Ok(Json(RegisterDeviceResponse {
        workspace_id,
        replica_id: request.replica_id,
        notification_schedule_revision: schedule_revision,
    }))
}

async fn notification_schedule_status(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(workspace_id): Path<WorkspaceId>,
) -> Result<Json<NotificationScheduleStatusResponse>, ApiError> {
    let user_id = authenticate(&state, &headers)?;
    let store = read_store(&state)?;
    authorize_workspace_owner(&store, workspace_id, user_id)?;

    let schedule = store.notification_schedules.get(&workspace_id);
    let schedule_revision = schedule.map(|schedule| schedule.revision).unwrap_or(0);
    let changed_at = schedule.and_then(|schedule| schedule.changed_at);
    let mut devices = store
        .devices
        .iter()
        .filter(|(key, _)| key.workspace_id == workspace_id && key.user_id == user_id)
        .map(|(key, device)| {
            let pending_background_push_count = store
                .background_push_outbox
                .iter()
                .filter(|push| {
                    push.workspace_id == workspace_id && push.target_replica_id == key.replica_id
                })
                .count();
            device_notification_status(
                key.replica_id,
                device,
                schedule_revision,
                pending_background_push_count,
            )
        })
        .collect::<Vec<_>>();
    devices.sort_by_key(|device| device.replica_id.to_string());

    let pending_background_pushes = store
        .background_push_outbox
        .iter()
        .filter(|push| push.workspace_id == workspace_id)
        .cloned()
        .collect();

    Ok(Json(NotificationScheduleStatusResponse {
        workspace_id,
        notification_schedule_revision: schedule_revision,
        changed_at,
        devices,
        pending_background_pushes,
    }))
}

async fn mark_notification_schedule_changed(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(workspace_id): Path<WorkspaceId>,
    Json(request): Json<MarkNotificationScheduleChangedRequest>,
) -> Result<Json<MarkNotificationScheduleChangedResponse>, ApiError> {
    let user_id = authenticate(&state, &headers)?;
    let mut store = write_store(&state)?;
    authorize_workspace_owner(&store, workspace_id, user_id)?;
    let key = DeviceKey {
        workspace_id,
        user_id,
        replica_id: request.replica_id,
    };
    if !store.devices.contains_key(&key) {
        return Err(ApiError::not_found());
    }

    let (notification_schedule_revision, background_pushes_enqueued) = mark_schedule_changed(
        &mut store,
        workspace_id,
        Some(request.replica_id),
        request.reason,
    );
    let now = Utc::now();
    let device = store
        .devices
        .get_mut(&key)
        .expect("registered device disappeared");
    apply_schedule_ack(
        device,
        notification_schedule_revision,
        request.scheduled_until,
        request.scheduled_occurrence_ids,
        request.notification_permission,
        request.local_scheduler_supported,
        now,
    );

    Ok(Json(MarkNotificationScheduleChangedResponse {
        workspace_id,
        replica_id: request.replica_id,
        notification_schedule_revision,
        background_pushes_enqueued,
    }))
}

async fn ack_notification_schedule(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(workspace_id): Path<WorkspaceId>,
    Json(request): Json<AckNotificationScheduleRequest>,
) -> Result<Json<AckNotificationScheduleResponse>, ApiError> {
    let user_id = authenticate(&state, &headers)?;
    let mut store = write_store(&state)?;
    authorize_workspace_owner(&store, workspace_id, user_id)?;
    let server_schedule_revision =
        current_notification_schedule_revision_mut(&mut store, workspace_id);
    if request.notification_schedule_revision > server_schedule_revision {
        return Err(ApiError::conflict("notification_schedule_revision_ahead"));
    }

    let key = DeviceKey {
        workspace_id,
        user_id,
        replica_id: request.replica_id,
    };
    let device = store
        .devices
        .get_mut(&key)
        .ok_or_else(ApiError::not_found)?;
    apply_schedule_ack(
        device,
        request.notification_schedule_revision,
        request.scheduled_until,
        request.scheduled_occurrence_ids,
        request.notification_permission,
        request.local_scheduler_supported,
        Utc::now(),
    );
    let accepted_revision = device.acknowledged_schedule_revision;
    drain_acknowledged_background_pushes(
        &mut store,
        workspace_id,
        request.replica_id,
        accepted_revision,
    );

    Ok(Json(AckNotificationScheduleResponse {
        workspace_id,
        replica_id: request.replica_id,
        accepted_revision,
        server_schedule_revision,
        up_to_date: accepted_revision >= server_schedule_revision,
    }))
}

async fn upsert_document(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((workspace_id, document)): Path<(WorkspaceId, DocumentId)>,
    Json(request): Json<knotq_sync::UpsertDocumentRequest>,
) -> Result<Json<DocumentResponse>, ApiError> {
    let user_id = authenticate(&state, &headers)?;
    let mut store = write_store(&state)?;
    let key = DocumentKey {
        workspace_id,
        document,
    };

    if let Some(existing) = store.documents.get(&key) {
        if existing.kind != request.kind {
            return Err(ApiError::conflict("document_kind_mismatch"));
        }
        let role = document_role(existing, user_id).ok_or_else(ApiError::forbidden)?;
        return Ok(Json(document_response(
            workspace_id,
            document,
            existing,
            role,
        )));
    }

    ensure_workspace_owner(&mut store, workspace_id, user_id)?;
    let record = DocumentRecord {
        kind: request.kind,
        owner: user_id,
        grants: HashMap::new(),
        shares: HashMap::new(),
        next_sequence: 1,
    };
    let response = document_response(workspace_id, document, &record, AccessRole::Owner);
    store.documents.insert(key, record);
    Ok(Json(response))
}

async fn push_updates(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((workspace_id, document)): Path<(WorkspaceId, DocumentId)>,
    Json(request): Json<PushUpdatesRequest>,
) -> Result<Json<PushUpdatesResponse>, ApiError> {
    let user_id = authenticate(&state, &headers)?;
    let mut store = write_store(&state)?;
    let key = DocumentKey {
        workspace_id,
        document,
    };

    let (latest_sequence, stored_updates) = {
        let record = store
            .documents
            .get_mut(&key)
            .ok_or_else(ApiError::not_found)?;
        let role = document_role(record, user_id).ok_or_else(ApiError::forbidden)?;
        if !role.can_write() {
            return Err(ApiError::forbidden());
        }
        validate_updates(document, record.kind, &request.updates)?;

        let mut stored_updates = Vec::with_capacity(request.updates.len());
        for update in request.updates {
            let sequence = record.next_sequence;
            record.next_sequence += 1;
            stored_updates.push(StoredCrdtUpdate {
                workspace_id,
                document: update.document,
                kind: update.kind,
                replica_id: request.replica_id,
                sequence,
                received_at: Utc::now(),
                update_v1: update.update_v1,
            });
        }
        let latest_sequence = record.next_sequence.saturating_sub(1);
        (latest_sequence, stored_updates)
    };

    let accepted = stored_updates.len();
    store.updates.entry(key).or_default().extend(stored_updates);
    let (notification_schedule_revision, background_pushes_enqueued) =
        if request.notification_schedule_changed {
            mark_schedule_changed(
                &mut store,
                workspace_id,
                Some(request.replica_id),
                NotificationScheduleChangeReason::SyncUpdate,
            )
        } else {
            (
                current_notification_schedule_revision_mut(&mut store, workspace_id),
                0,
            )
        };

    Ok(Json(PushUpdatesResponse {
        accepted,
        latest_sequence,
        notification_schedule_revision,
        background_pushes_enqueued,
    }))
}

async fn pull_updates(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((workspace_id, document)): Path<(WorkspaceId, DocumentId)>,
    Query(query): Query<PullUpdatesQuery>,
) -> Result<Json<PullUpdatesResponse>, ApiError> {
    let user_id = authenticate(&state, &headers)?;
    let store = read_store(&state)?;
    let key = DocumentKey {
        workspace_id,
        document,
    };
    let record = store.documents.get(&key).ok_or_else(ApiError::not_found)?;
    let role = document_role(record, user_id).ok_or_else(ApiError::forbidden)?;
    if !role.can_read() {
        return Err(ApiError::forbidden());
    }

    let all_updates = store.updates.get(&key).map(Vec::as_slice).unwrap_or(&[]);
    let latest_sequence = all_updates
        .last()
        .map(|update| update.sequence)
        .unwrap_or(0);
    let updates = all_updates
        .iter()
        .filter(|update| update.sequence > query.after)
        .filter(|update| query.exclude_replica != Some(update.replica_id))
        .cloned()
        .collect();
    let notification_schedule_revision =
        current_notification_schedule_revision(&store, workspace_id);

    Ok(Json(PullUpdatesResponse {
        updates,
        latest_sequence,
        notification_schedule_revision,
    }))
}

async fn share_document(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((workspace_id, document)): Path<(WorkspaceId, DocumentId)>,
    Json(request): Json<ShareDocumentRequest>,
) -> Result<Json<ShareDocumentResponse>, ApiError> {
    let user_id = authenticate(&state, &headers)?;
    if request.role == AccessRole::Owner {
        return Err(ApiError::bad_request("share_role_cannot_be_owner"));
    }

    let mut store = write_store(&state)?;
    let key = DocumentKey {
        workspace_id,
        document,
    };
    let record = store
        .documents
        .get_mut(&key)
        .ok_or_else(ApiError::not_found)?;
    if record.owner != user_id {
        return Err(ApiError::forbidden());
    }

    let share_id = ShareId::new();
    record.grants.insert(request.grantee, request.role);
    record.shares.insert(
        share_id,
        ShareGrant {
            grantee: request.grantee,
            role: request.role,
        },
    );
    let grant = record
        .shares
        .get(&share_id)
        .expect("inserted share grant is missing");

    Ok(Json(ShareDocumentResponse {
        workspace_id,
        document,
        share_id,
        grantee: grant.grantee,
        role: grant.role,
    }))
}

fn authenticate(state: &AppState, headers: &HeaderMap) -> Result<UserId, ApiError> {
    let Some(value) = headers.get(axum::http::header::AUTHORIZATION) else {
        return Err(ApiError::unauthorized());
    };
    let value = value.to_str().map_err(|_| ApiError::unauthorized())?;
    let Some(token) = value.strip_prefix("Bearer ") else {
        return Err(ApiError::unauthorized());
    };
    read_store(state)?
        .sessions
        .get(token)
        .copied()
        .ok_or_else(ApiError::unauthorized)
}

fn ensure_workspace_owner(
    store: &mut BackendStore,
    workspace_id: WorkspaceId,
    user_id: UserId,
) -> Result<(), ApiError> {
    match store.workspace_owners.get(&workspace_id).copied() {
        Some(owner) if owner == user_id => Ok(()),
        Some(_) => Err(ApiError::forbidden()),
        None => {
            store.workspace_owners.insert(workspace_id, user_id);
            Ok(())
        }
    }
}

fn authorize_workspace_owner(
    store: &BackendStore,
    workspace_id: WorkspaceId,
    user_id: UserId,
) -> Result<(), ApiError> {
    match store.workspace_owners.get(&workspace_id).copied() {
        Some(owner) if owner == user_id => Ok(()),
        Some(_) => Err(ApiError::forbidden()),
        None => Err(ApiError::not_found()),
    }
}

fn validate_push_registration(request: &RegisterDeviceRequest) -> Result<(), ApiError> {
    let has_push_token = request
        .push_token
        .as_deref()
        .map(|token| !token.trim().is_empty())
        .unwrap_or(false);
    if has_push_token && request.push_channel.is_none() {
        return Err(ApiError::bad_request("push_channel_required"));
    }
    Ok(())
}

fn validate_updates(
    document: DocumentId,
    kind: SyncDocumentKind,
    updates: &[CrdtDocumentUpdate],
) -> Result<(), ApiError> {
    if updates.is_empty() {
        return Err(ApiError::bad_request("updates_empty"));
    }
    for update in updates {
        if update.document != document {
            return Err(ApiError::bad_request("update_document_mismatch"));
        }
        if update.kind != kind {
            return Err(ApiError::bad_request("update_kind_mismatch"));
        }
        if update.update_v1.is_empty() {
            return Err(ApiError::bad_request("update_payload_empty"));
        }
    }
    Ok(())
}

fn clean_optional_string(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn current_notification_schedule_revision(store: &BackendStore, workspace_id: WorkspaceId) -> u64 {
    store
        .notification_schedules
        .get(&workspace_id)
        .map(|schedule| schedule.revision)
        .unwrap_or(0)
}

fn current_notification_schedule_revision_mut(
    store: &mut BackendStore,
    workspace_id: WorkspaceId,
) -> u64 {
    store
        .notification_schedules
        .entry(workspace_id)
        .or_default()
        .revision
}

fn mark_schedule_changed(
    store: &mut BackendStore,
    workspace_id: WorkspaceId,
    origin_replica_id: Option<ReplicaId>,
    reason: NotificationScheduleChangeReason,
) -> (u64, usize) {
    let now = Utc::now();
    let schedule = store
        .notification_schedules
        .entry(workspace_id)
        .or_default();
    schedule.revision = schedule.revision.saturating_add(1);
    schedule.changed_at = Some(now);
    let revision = schedule.revision;

    let target_keys = store
        .devices
        .iter()
        .filter(|(key, device)| {
            key.workspace_id == workspace_id
                && Some(key.replica_id) != origin_replica_id
                && device.push_channel.is_some()
                && device.push_token.is_some()
        })
        .map(|(key, _)| *key)
        .collect::<Vec<_>>();

    let push_count = target_keys.len();
    for key in target_keys {
        let push = BackgroundPushIntent {
            id: store.next_background_push_id,
            workspace_id,
            target_replica_id: key.replica_id,
            notification_schedule_revision: revision,
            kind: BackgroundPushKind::NotificationScheduleChanged,
            reason,
            enqueued_at: now,
        };
        store.next_background_push_id = store.next_background_push_id.saturating_add(1);
        store.background_push_outbox.push(push);
        if let Some(device) = store.devices.get_mut(&key) {
            device.last_background_push_enqueued_at = Some(now);
        }
    }

    (revision, push_count)
}

fn apply_schedule_ack(
    device: &mut DeviceRecord,
    revision: u64,
    scheduled_until: Option<DateTime<Utc>>,
    scheduled_occurrence_ids: Vec<String>,
    notification_permission: Option<NotificationPermissionState>,
    local_scheduler_supported: Option<bool>,
    now: DateTime<Utc>,
) {
    device.last_ack_at = Some(now);
    device.updated_at = now;
    if let Some(notification_permission) = notification_permission {
        device.notification_permission = notification_permission;
    }
    if local_scheduler_supported.is_some() {
        device.local_scheduler_supported = local_scheduler_supported;
    }
    if revision >= device.acknowledged_schedule_revision {
        device.acknowledged_schedule_revision = revision;
        device.scheduled_until = scheduled_until;
        device.scheduled_occurrence_ids = scheduled_occurrence_ids;
    }
}

fn drain_acknowledged_background_pushes(
    store: &mut BackendStore,
    workspace_id: WorkspaceId,
    replica_id: ReplicaId,
    acknowledged_revision: u64,
) {
    store.background_push_outbox.retain(|push| {
        !(push.workspace_id == workspace_id
            && push.target_replica_id == replica_id
            && push.notification_schedule_revision <= acknowledged_revision)
    });
}

fn device_notification_status(
    replica_id: ReplicaId,
    device: &DeviceRecord,
    server_schedule_revision: u64,
    pending_background_push_count: usize,
) -> DeviceNotificationScheduleStatus {
    DeviceNotificationScheduleStatus {
        replica_id,
        display_name: device.display_name.clone(),
        platform: device.platform,
        app_version: device.app_version.clone(),
        push_channel: device.push_channel,
        has_push_token: device.push_token.is_some(),
        push_environment: device.push_environment,
        notification_permission: device.notification_permission,
        local_scheduler_supported: device.local_scheduler_supported,
        acknowledged_schedule_revision: device.acknowledged_schedule_revision,
        last_ack_at: device.last_ack_at,
        scheduled_until: device.scheduled_until,
        scheduled_occurrence_count: device.scheduled_occurrence_ids.len(),
        schedule_status: schedule_device_state(device, server_schedule_revision),
        pending_background_push_count,
        last_background_push_enqueued_at: device.last_background_push_enqueued_at,
        registered_at: device.registered_at,
        updated_at: device.updated_at,
    }
}

fn schedule_device_state(
    device: &DeviceRecord,
    server_schedule_revision: u64,
) -> NotificationScheduleDeviceState {
    if device.notification_permission == NotificationPermissionState::Denied {
        return NotificationScheduleDeviceState::PermissionDenied;
    }
    if device.local_scheduler_supported == Some(false) {
        return NotificationScheduleDeviceState::LocalSchedulingUnavailable;
    }
    if device.last_ack_at.is_none() {
        return NotificationScheduleDeviceState::Unknown;
    }
    if device.acknowledged_schedule_revision >= server_schedule_revision {
        NotificationScheduleDeviceState::Fresh
    } else {
        NotificationScheduleDeviceState::Stale
    }
}

fn document_role(record: &DocumentRecord, user_id: UserId) -> Option<AccessRole> {
    if record.owner == user_id {
        Some(AccessRole::Owner)
    } else {
        record.grants.get(&user_id).copied()
    }
}

fn document_response(
    workspace_id: WorkspaceId,
    document: DocumentId,
    record: &DocumentRecord,
    role: AccessRole,
) -> DocumentResponse {
    DocumentResponse {
        workspace_id,
        document,
        kind: record.kind,
        owner: record.owner,
        role,
    }
}

fn normalize_email(email: &str) -> Result<String, ApiError> {
    let email = email.trim().to_ascii_lowercase();
    if email.is_empty() || !email.contains('@') {
        return Err(ApiError::bad_request("invalid_email"));
    }
    Ok(email)
}

fn make_session_token(email: &str, user_id: UserId) -> String {
    let mut nonce = [0_u8; 32];
    rand::thread_rng().fill_bytes(&mut nonce);
    let mut hasher = Sha256::new();
    hasher.update(email.as_bytes());
    hasher.update(user_id.to_string().as_bytes());
    hasher.update(nonce);
    format!("{:x}", hasher.finalize())
}

fn read_store(state: &AppState) -> Result<std::sync::RwLockReadGuard<'_, BackendStore>, ApiError> {
    state.inner.read().map_err(|_| ApiError::internal())
}

fn write_store(
    state: &AppState,
) -> Result<std::sync::RwLockWriteGuard<'_, BackendStore>, ApiError> {
    state.inner.write().map_err(|_| ApiError::internal())
}

struct ApiError {
    status: StatusCode,
    code: &'static str,
}

impl ApiError {
    fn bad_request(code: &'static str) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code,
        }
    }

    fn unauthorized() -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            code: "unauthorized",
        }
    }

    fn forbidden() -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            code: "forbidden",
        }
    }

    fn not_found() -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: "not_found",
        }
    }

    fn conflict(code: &'static str) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code,
        }
    }

    fn internal() -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal_error",
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorResponse::new(self.code, self.code.replace('_', " "))),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{header, Method, Request},
    };
    use http_body_util::BodyExt;
    use knotq_sync::{
        AccessRole, AckNotificationScheduleRequest, AckNotificationScheduleResponse,
        DevLoginRequest, DevicePlatform, MarkNotificationScheduleChangedRequest,
        MarkNotificationScheduleChangedResponse, NotificationPermissionState,
        NotificationScheduleChangeReason, NotificationScheduleDeviceState,
        NotificationScheduleStatusResponse, PushChannel, PushEnvironment, RegisterDeviceRequest,
        RegisterDeviceResponse, UpsertDocumentRequest,
    };
    use serde::de::DeserializeOwned;
    use tower::ServiceExt;

    #[tokio::test]
    async fn private_document_rejects_other_users() {
        let app = app();
        let alice = login(&app, "alice@example.com").await;
        let bob = login(&app, "bob@example.com").await;
        let workspace_id = WorkspaceId::new();
        let document = DocumentId::new();

        let response = send_json(
            &app,
            Method::PUT,
            &format!("/v1/workspaces/{}/documents/{}", workspace_id, document),
            Some(&alice.bearer_token),
            &UpsertDocumentRequest {
                kind: SyncDocumentKind::Scheme,
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        let response = send_json(
            &app,
            Method::POST,
            &format!(
                "/v1/workspaces/{}/documents/{}/updates",
                workspace_id, document
            ),
            Some(&bob.bearer_token),
            &PushUpdatesRequest {
                replica_id: ReplicaId::new(),
                notification_schedule_changed: false,
                updates: vec![CrdtDocumentUpdate {
                    document,
                    kind: SyncDocumentKind::Scheme,
                    update_v1: vec![1, 2, 3],
                }],
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn shared_document_allows_grantee_to_pull_updates() {
        let app = app();
        let alice = login(&app, "alice@example.com").await;
        let bob = login(&app, "bob@example.com").await;
        let workspace_id = WorkspaceId::new();
        let document = DocumentId::new();
        let replica_id = ReplicaId::new();

        assert_eq!(
            send_json(
                &app,
                Method::PUT,
                &format!("/v1/workspaces/{}/documents/{}", workspace_id, document),
                Some(&alice.bearer_token),
                &UpsertDocumentRequest {
                    kind: SyncDocumentKind::Scheme,
                },
            )
            .await
            .status(),
            StatusCode::OK
        );

        assert_eq!(
            send_json(
                &app,
                Method::POST,
                &format!(
                    "/v1/workspaces/{}/documents/{}/shares",
                    workspace_id, document
                ),
                Some(&alice.bearer_token),
                &ShareDocumentRequest {
                    grantee: bob.user_id,
                    role: AccessRole::Reader,
                },
            )
            .await
            .status(),
            StatusCode::OK
        );
        let document_response: DocumentResponse = read_json(
            send_json(
                &app,
                Method::PUT,
                &format!("/v1/workspaces/{}/documents/{}", workspace_id, document),
                Some(&bob.bearer_token),
                &UpsertDocumentRequest {
                    kind: SyncDocumentKind::Scheme,
                },
            )
            .await,
        )
        .await;
        assert_eq!(document_response.role, AccessRole::Reader);

        let push: PushUpdatesResponse = read_json(
            send_json(
                &app,
                Method::POST,
                &format!(
                    "/v1/workspaces/{}/documents/{}/updates",
                    workspace_id, document
                ),
                Some(&alice.bearer_token),
                &PushUpdatesRequest {
                    replica_id,
                    notification_schedule_changed: false,
                    updates: vec![CrdtDocumentUpdate {
                        document,
                        kind: SyncDocumentKind::Scheme,
                        update_v1: vec![9, 8, 7],
                    }],
                },
            )
            .await,
        )
        .await;
        assert_eq!(push.accepted, 1);

        let pull: PullUpdatesResponse = read_json(
            send_empty(
                &app,
                Method::GET,
                &format!(
                    "/v1/workspaces/{}/documents/{}/updates",
                    workspace_id, document
                ),
                Some(&bob.bearer_token),
            )
            .await,
        )
        .await;
        assert_eq!(pull.updates.len(), 1);
        assert_eq!(pull.updates[0].update_v1, vec![9, 8, 7]);
    }

    #[tokio::test]
    async fn notification_schedule_change_acks_origin_and_queues_background_push() {
        let app = app();
        let alice = login(&app, "alice@example.com").await;
        let workspace_id = WorkspaceId::new();
        let phone = ReplicaId::new();
        let tablet = ReplicaId::new();

        register_device(
            &app,
            &alice.bearer_token,
            workspace_id,
            phone,
            Some("Phone"),
            Some("apns-phone-token"),
        )
        .await;
        register_device(
            &app,
            &alice.bearer_token,
            workspace_id,
            tablet,
            Some("Tablet"),
            Some("apns-tablet-token"),
        )
        .await;

        let changed: MarkNotificationScheduleChangedResponse = read_json(
            send_json(
                &app,
                Method::POST,
                &format!(
                    "/v1/workspaces/{}/notification-schedule/changes",
                    workspace_id
                ),
                Some(&alice.bearer_token),
                &MarkNotificationScheduleChangedRequest {
                    replica_id: phone,
                    reason: NotificationScheduleChangeReason::ReminderChanged,
                    scheduled_until: Some(Utc::now()),
                    scheduled_occurrence_ids: vec!["reminder:item-1:single".to_string()],
                    notification_permission: Some(NotificationPermissionState::Granted),
                    local_scheduler_supported: Some(true),
                },
            )
            .await,
        )
        .await;
        assert_eq!(changed.notification_schedule_revision, 1);
        assert_eq!(changed.background_pushes_enqueued, 1);

        let status: NotificationScheduleStatusResponse = read_json(
            send_empty(
                &app,
                Method::GET,
                &format!("/v1/workspaces/{}/notification-schedule", workspace_id),
                Some(&alice.bearer_token),
            )
            .await,
        )
        .await;
        assert_eq!(status.notification_schedule_revision, 1);
        assert_eq!(status.pending_background_pushes.len(), 1);
        assert_eq!(
            status.pending_background_pushes[0].target_replica_id,
            tablet
        );

        let phone_status = status
            .devices
            .iter()
            .find(|device| device.replica_id == phone)
            .expect("phone status missing");
        assert_eq!(
            phone_status.schedule_status,
            NotificationScheduleDeviceState::Fresh
        );
        assert_eq!(phone_status.acknowledged_schedule_revision, 1);
        assert_eq!(phone_status.scheduled_occurrence_count, 1);
        assert_eq!(phone_status.pending_background_push_count, 0);

        let tablet_status = status
            .devices
            .iter()
            .find(|device| device.replica_id == tablet)
            .expect("tablet status missing");
        assert_eq!(
            tablet_status.schedule_status,
            NotificationScheduleDeviceState::Unknown
        );
        assert_eq!(tablet_status.pending_background_push_count, 1);
    }

    #[tokio::test]
    async fn notification_schedule_ack_marks_device_fresh_and_drains_push() {
        let app = app();
        let alice = login(&app, "alice@example.com").await;
        let workspace_id = WorkspaceId::new();
        let phone = ReplicaId::new();
        let tablet = ReplicaId::new();

        register_device(
            &app,
            &alice.bearer_token,
            workspace_id,
            phone,
            Some("Phone"),
            Some("apns-phone-token"),
        )
        .await;
        register_device(
            &app,
            &alice.bearer_token,
            workspace_id,
            tablet,
            Some("Tablet"),
            Some("apns-tablet-token"),
        )
        .await;

        let changed: MarkNotificationScheduleChangedResponse = read_json(
            send_json(
                &app,
                Method::POST,
                &format!(
                    "/v1/workspaces/{}/notification-schedule/changes",
                    workspace_id
                ),
                Some(&alice.bearer_token),
                &MarkNotificationScheduleChangedRequest {
                    replica_id: phone,
                    reason: NotificationScheduleChangeReason::ReminderChanged,
                    scheduled_until: Some(Utc::now()),
                    scheduled_occurrence_ids: vec!["reminder:item-1:single".to_string()],
                    notification_permission: Some(NotificationPermissionState::Granted),
                    local_scheduler_supported: Some(true),
                },
            )
            .await,
        )
        .await;

        let ack: AckNotificationScheduleResponse = read_json(
            send_json(
                &app,
                Method::POST,
                &format!("/v1/workspaces/{}/notification-schedule/ack", workspace_id),
                Some(&alice.bearer_token),
                &AckNotificationScheduleRequest {
                    replica_id: tablet,
                    notification_schedule_revision: changed.notification_schedule_revision,
                    scheduled_until: Some(Utc::now()),
                    scheduled_occurrence_ids: vec!["reminder:item-1:single".to_string()],
                    notification_permission: Some(NotificationPermissionState::Granted),
                    local_scheduler_supported: Some(true),
                },
            )
            .await,
        )
        .await;
        assert!(ack.up_to_date);
        assert_eq!(
            ack.accepted_revision,
            changed.notification_schedule_revision
        );

        let status: NotificationScheduleStatusResponse = read_json(
            send_empty(
                &app,
                Method::GET,
                &format!("/v1/workspaces/{}/notification-schedule", workspace_id),
                Some(&alice.bearer_token),
            )
            .await,
        )
        .await;
        assert!(status.pending_background_pushes.is_empty());
        let tablet_status = status
            .devices
            .iter()
            .find(|device| device.replica_id == tablet)
            .expect("tablet status missing");
        assert_eq!(
            tablet_status.schedule_status,
            NotificationScheduleDeviceState::Fresh
        );
        assert_eq!(tablet_status.pending_background_push_count, 0);
    }

    async fn login(app: &Router, email: &str) -> AuthSession {
        read_json(
            send_json(
                app,
                Method::POST,
                "/v1/auth/dev-login",
                None,
                &DevLoginRequest {
                    email: email.to_string(),
                },
            )
            .await,
        )
        .await
    }

    async fn register_device(
        app: &Router,
        token: &str,
        workspace_id: WorkspaceId,
        replica_id: ReplicaId,
        display_name: Option<&str>,
        push_token: Option<&str>,
    ) -> RegisterDeviceResponse {
        read_json(
            send_json(
                app,
                Method::POST,
                &format!("/v1/workspaces/{}/devices", workspace_id),
                Some(token),
                &RegisterDeviceRequest {
                    replica_id,
                    display_name: display_name.map(str::to_string),
                    platform: DevicePlatform::Ios,
                    app_version: Some("1.0.0".to_string()),
                    push_channel: push_token.map(|_| PushChannel::Apns),
                    push_token: push_token.map(str::to_string),
                    push_environment: Some(PushEnvironment::Sandbox),
                    notification_permission: NotificationPermissionState::Granted,
                    local_scheduler_supported: Some(true),
                },
            )
            .await,
        )
        .await
    }

    async fn send_json<T: Serialize>(
        app: &Router,
        method: Method,
        uri: &str,
        token: Option<&str>,
        body: &T,
    ) -> axum::response::Response {
        let mut builder = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(token) = token {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        let body = serde_json::to_vec(body).unwrap();
        app.clone()
            .oneshot(builder.body(Body::from(body)).unwrap())
            .await
            .unwrap()
    }

    async fn send_empty(
        app: &Router,
        method: Method,
        uri: &str,
        token: Option<&str>,
    ) -> axum::response::Response {
        let mut builder = Request::builder().method(method).uri(uri);
        if let Some(token) = token {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        app.clone()
            .oneshot(builder.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn read_json<T: DeserializeOwned>(response: axum::response::Response) -> T {
        assert!(
            response.status().is_success(),
            "unexpected status: {}",
            response.status()
        );
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }
}
