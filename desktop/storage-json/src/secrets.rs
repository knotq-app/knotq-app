//! Best-effort storage of auth secrets in the OS secure store — macOS Keychain,
//! Windows Credential Manager, or Linux Secret Service — via the `keyring` crate.
//!
//! The rest of the app never touches this module: `settings.rs` strips the secret
//! fields out of `AppSettings` before writing `settings.json` and rehydrates them
//! from here on load. Every function returns a `Result`, but callers treat any
//! error as "keychain unavailable" and fall back to keeping the secret in the JSON
//! file so a session is never lost (headless Linux without a Secret Service, a
//! locked keychain, CI, etc.).
//!
//! Set `KNOTQ_DISABLE_KEYCHAIN=1` to bypass the keychain entirely (plaintext in
//! the settings file, the pre-keychain behavior). The storage round-trip tests use
//! `keyring`'s in-memory mock store instead.

use anyhow::{anyhow, Result};
#[cfg(not(test))]
use keyring::Entry;
use serde::{Deserialize, Serialize};

/// Keychain service name; all KnotQ secrets live under this service. (Only the
/// real keyring backend uses it; the test backend keys an in-memory map.)
#[cfg_attr(test, allow(dead_code))]
const SERVICE: &str = "KnotQ";
/// Account name for the single sync credential bundle.
const SYNC_USER: &str = "sync";

/// Sync access + refresh tokens for the (single) signed-in sync account.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncSecret {
    #[serde(default)]
    pub bearer: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh: Option<String>,
}

/// Google OAuth access + refresh tokens for one linked account.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleSecret {
    #[serde(default)]
    pub access: String,
    #[serde(default)]
    pub refresh: String,
}

/// Whether the OS keychain should be used. `KNOTQ_DISABLE_KEYCHAIN` set to a
/// truthy value forces plaintext-in-file behavior (CI, headless boxes, tests).
pub fn is_enabled() -> bool {
    match std::env::var("KNOTQ_DISABLE_KEYCHAIN") {
        Ok(value) => !matches!(value.trim(), "1" | "true" | "yes"),
        Err(_) => true,
    }
}

fn google_user(account_id: &str) -> String {
    format!("google:{account_id}")
}

#[cfg(not(test))]
fn store(user: &str, payload: &str) -> Result<()> {
    Entry::new(SERVICE, user)?.set_password(payload)?;
    Ok(())
}

#[cfg(not(test))]
fn load(user: &str) -> Result<String> {
    Ok(Entry::new(SERVICE, user)?.get_password()?)
}

#[cfg(not(test))]
fn delete(user: &str) -> Result<()> {
    match Entry::new(SERVICE, user)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(err) => Err(err.into()),
    }
}

// Tests can't use a real OS keychain (non-hermetic, prompts, headless CI) and the
// `keyring` mock store is per-`Entry`, so it can't round-trip a value stored under
// one `Entry` and read under another. Route the backend through a process-global
// in-memory map instead, which faithfully exercises the redact/rehydrate/migrate
// logic in `settings.rs`.
#[cfg(test)]
fn test_store() -> &'static std::sync::Mutex<std::collections::HashMap<String, String>> {
    use std::sync::{Mutex, OnceLock};
    static STORE: OnceLock<Mutex<std::collections::HashMap<String, String>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

#[cfg(test)]
fn store(user: &str, payload: &str) -> Result<()> {
    test_store()
        .lock()
        .unwrap()
        .insert(user.to_string(), payload.to_string());
    Ok(())
}

#[cfg(test)]
fn load(user: &str) -> Result<String> {
    test_store()
        .lock()
        .unwrap()
        .get(user)
        .cloned()
        .ok_or_else(|| anyhow!("no entry"))
}

#[cfg(test)]
fn delete(user: &str) -> Result<()> {
    test_store().lock().unwrap().remove(user);
    Ok(())
}

pub fn store_sync(secret: &SyncSecret) -> Result<()> {
    if !is_enabled() {
        return Err(anyhow!("keychain disabled"));
    }
    store(SYNC_USER, &serde_json::to_string(secret)?)
}

pub fn load_sync() -> Result<SyncSecret> {
    if !is_enabled() {
        return Err(anyhow!("keychain disabled"));
    }
    Ok(serde_json::from_str(&load(SYNC_USER)?)?)
}

pub fn delete_sync() -> Result<()> {
    if !is_enabled() {
        return Ok(());
    }
    delete(SYNC_USER)
}

pub fn store_google(account_id: &str, secret: &GoogleSecret) -> Result<()> {
    if !is_enabled() {
        return Err(anyhow!("keychain disabled"));
    }
    store(&google_user(account_id), &serde_json::to_string(secret)?)
}

pub fn load_google(account_id: &str) -> Result<GoogleSecret> {
    if !is_enabled() {
        return Err(anyhow!("keychain disabled"));
    }
    Ok(serde_json::from_str(&load(&google_user(account_id))?)?)
}

pub fn delete_google(account_id: &str) -> Result<()> {
    if !is_enabled() {
        return Ok(());
    }
    delete(&google_user(account_id))
}
