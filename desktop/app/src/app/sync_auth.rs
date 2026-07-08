use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use serde::Deserialize;

mod account;
mod flow;
mod tokens;

pub(crate) use tokens::{refresh_sync_backend, RefreshError};

const PROD_SYNC_API_BASE: &str = "https://api.knotq.com";
const SANDBOX_SYNC_API_BASE: &str = "https://sandbox.api.knotq.com";

/// The default backend for a *new* sign-in when no `KNOTQ_API_BASE` override is
/// set: release (production) builds talk to prod, dev/debug builds talk to the
/// hosted sandbox so day-to-day development never touches production data.
const fn build_default_sync_api_base() -> &'static str {
    if cfg!(debug_assertions) {
        SANDBOX_SYNC_API_BASE
    } else {
        PROD_SYNC_API_BASE
    }
}

/// Base URL used for a *new* sign-in when no account is stored yet. An explicit
/// `KNOTQ_API_BASE` env override always wins (e.g. point a build at a local
/// Worker, `http://127.0.0.1:8787`); otherwise the default is build-aware — see
/// [`build_default_sync_api_base`]. Existing accounts keep their stored
/// `api_base`, so this never silently moves a signed-in account between
/// environments.
fn default_sync_api_base() -> String {
    match std::env::var("KNOTQ_API_BASE") {
        Ok(value) if !value.trim().is_empty() => value.trim().to_string(),
        _ => build_default_sync_api_base().to_string(),
    }
}

/// The KnotQ marketing/auth site origins. The hosted sign-in, account, and
/// checkout pages live here; the app opens whichever matches the backend it is
/// configured for (see [`sync_web_base`]).
const PROD_WEB_BASE: &str = "https://www.knotq.com";
const SANDBOX_WEB_BASE: &str = "https://sandbox.knotq.com";

/// The knotq.com site origin matching a given sync API base, so a sandbox or
/// local-dev build opens the sandbox site instead of production. The sign-in,
/// account, and checkout pages are *also* given the API base via the allowlisted
/// `?api=` param, which is what actually pins the backend — necessary for local
/// dev, where the site is the sandbox host but the API is the loopback Worker.
fn sync_web_base(api_base: &str) -> &'static str {
    if api_base.contains("sandbox.api.knotq.com")
        || api_base.contains("127.0.0.1")
        || api_base.contains("localhost")
    {
        SANDBOX_WEB_BASE
    } else {
        PROD_WEB_BASE
    }
}

#[derive(Deserialize)]
struct LoginResponse {
    #[serde(default)]
    session_id: Option<String>,
    user_id: String,
    workspace_id: String,
    email: String,
    #[serde(default = "default_supports_sync")]
    supports_sync: bool,
    bearer_token: String,
    expires_at: DateTime<Utc>,
    refresh_token: String,
    #[serde(default)]
    refresh_expires_at: Option<DateTime<Utc>>,
}

#[derive(Deserialize)]
struct CheckoutResponse {
    checkout_url: String,
}

#[derive(Deserialize)]
struct LoginError {
    code: String,
}

/// Percent-encode a query value, escaping everything outside the RFC 3986
/// unreserved set.
fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn default_supports_sync() -> bool {
    true
}

fn normalize_api_base(raw: &str) -> Result<String> {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err(anyhow!(knotq_l10n::t("sync.error.api_url_required")));
    }
    if let Some(after_scheme) = trimmed.strip_prefix("http://") {
        // Bearer tokens must never travel in cleartext, so plain http is only
        // permitted to a loopback host (local dev / self-hosted Worker on-box).
        if !is_loopback_http_authority(after_scheme) {
            return Err(anyhow!(knotq_l10n::t(
                "sync.error.api_url_https_required"
            )));
        }
    } else if trimmed.strip_prefix("https://").is_none() {
        return Err(anyhow!(knotq_l10n::t(
            "sync.error.api_url_scheme_required"
        )));
    }
    Ok(trimmed.to_string())
}

/// Whether the authority following `http://` names a loopback host. Accepts
/// `127.0.0.1`, `localhost`, and the IPv6 literal `[::1]`, with or without a port
/// or trailing path.
fn is_loopback_http_authority(after_scheme: &str) -> bool {
    let authority = after_scheme.split('/').next().unwrap_or("");
    // Defensive: drop any userinfo ("user:pass@host").
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    let host = if let Some(rest) = authority.strip_prefix('[') {
        // IPv6 literal: the host is inside the brackets.
        rest.split(']').next().unwrap_or("")
    } else {
        authority.split(':').next().unwrap_or("")
    };
    matches!(
        host.to_ascii_lowercase().as_str(),
        "127.0.0.1" | "localhost" | "::1"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_web_base_matches_backend() {
        // Production API → production site.
        assert_eq!(sync_web_base("https://api.knotq.com"), PROD_WEB_BASE);
        // Sandbox and local-dev backends → the sandbox site (it can target a
        // local API via the allowlisted `?api=` param).
        assert_eq!(
            sync_web_base("https://sandbox.api.knotq.com"),
            SANDBOX_WEB_BASE
        );
        assert_eq!(sync_web_base("http://127.0.0.1:8787"), SANDBOX_WEB_BASE);
        assert_eq!(sync_web_base("http://localhost:8787"), SANDBOX_WEB_BASE);
    }

    #[test]
    fn normalize_api_base_accepts_https() {
        assert_eq!(
            normalize_api_base("https://sync.example.com").unwrap(),
            "https://sync.example.com"
        );
        // Trailing slashes are trimmed.
        assert_eq!(
            normalize_api_base("https://sync.example.com/").unwrap(),
            "https://sync.example.com"
        );
    }

    #[test]
    fn normalize_api_base_allows_http_only_for_loopback() {
        assert_eq!(
            normalize_api_base("http://127.0.0.1:8787").unwrap(),
            "http://127.0.0.1:8787"
        );
        assert_eq!(
            normalize_api_base("http://localhost:8787").unwrap(),
            "http://localhost:8787"
        );
        assert_eq!(
            normalize_api_base("http://[::1]:8787").unwrap(),
            "http://[::1]:8787"
        );
    }

    #[test]
    fn normalize_api_base_rejects_plain_http_to_remote_host() {
        assert!(normalize_api_base("http://example.com").is_err());
        assert!(normalize_api_base("http://sync.example.com:8787").is_err());
        // A loopback-looking name embedded elsewhere must not slip through.
        assert!(normalize_api_base("http://localhost.evil.com").is_err());
    }

    #[test]
    fn normalize_api_base_rejects_other_schemes_and_empty() {
        assert!(normalize_api_base("ftp://example.com").is_err());
        assert!(normalize_api_base("example.com").is_err());
        assert!(normalize_api_base("   ").is_err());
    }
}
