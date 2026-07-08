use std::time::Duration as StdDuration;

use anyhow::{anyhow, Context as AnyhowContext, Result};
use chrono::{DateTime, Utc};
use knotq_model::SyncAccountSettings;

use super::{normalize_api_base, LoginResponse};

/// Fresh credentials returned by `POST /v1/auth/refresh`.
pub(crate) struct RefreshedTokens {
    pub bearer_token: String,
    pub expires_at: DateTime<Utc>,
    pub refresh_token: String,
    pub refresh_expires_at: Option<DateTime<Utc>>,
    pub workspace_id: String,
    pub supports_sync: bool,
}

/// Why a refresh attempt failed. `Unauthorized` means the refresh token is dead
/// (revoked/expired/replayed) and the user must sign in again; `Transient` is a
/// network/parse hiccup worth retrying on the next sync tick.
pub(crate) enum RefreshError {
    Unauthorized,
    Transient(anyhow::Error),
}

/// Exchange a refresh token for a fresh access token (and a rotated refresh
/// token). Runs on a background thread (blocking `ureq`).
pub(crate) fn refresh_sync_backend(
    api_base: &str,
    refresh_token: &str,
) -> Result<RefreshedTokens, RefreshError> {
    let base = normalize_api_base(api_base).map_err(RefreshError::Transient)?;
    let url = format!("{base}/v1/auth/refresh");
    let response = match ureq::post(&url)
        .timeout(StdDuration::from_secs(10))
        .send_json(serde_json::json!({ "refresh_token": refresh_token }))
    {
        Ok(response) => response,
        Err(ureq::Error::Status(401, _)) => return Err(RefreshError::Unauthorized),
        Err(error) => {
            return Err(RefreshError::Transient(anyhow!(knotq_l10n::t_with(
                "sync.error.refresh_failed",
                &[("error", &error.to_string())],
            ))))
        }
    };
    let session: LoginResponse = response
        .into_json()
        .context("parse sync refresh response")
        .map_err(RefreshError::Transient)?;
    if session.refresh_token.is_empty() {
        return Err(RefreshError::Transient(anyhow!(knotq_l10n::t(
            "sync.error.refresh_missing_token"
        ))));
    }
    Ok(RefreshedTokens {
        bearer_token: session.bearer_token,
        expires_at: session.expires_at,
        refresh_token: session.refresh_token,
        refresh_expires_at: session.refresh_expires_at,
        workspace_id: session.workspace_id,
        supports_sync: session.supports_sync,
    })
}

pub(super) fn logout_sync_backend(account: &SyncAccountSettings) -> Result<()> {
    let url = format!("{}/v1/auth/logout", normalize_api_base(&account.api_base)?);
    match ureq::post(&url)
        .timeout(StdDuration::from_secs(5))
        .set("authorization", &format!("Bearer {}", account.bearer_token))
        .send_json(serde_json::json!({}))
    {
        Ok(_) | Err(ureq::Error::Status(401, _)) => Ok(()),
        Err(error) => Err(anyhow!("Could not revoke sync session: {error}")),
    }
}
