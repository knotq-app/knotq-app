//! Durable, one-shot capabilities for Windows notification protocol actions.
//!
//! A custom URI is public input on Windows.  Keeping the capability ledger in a
//! separate module makes the security decision testable on every host, while
//! the Windows backend owns the location of the ledger on disk.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::Mutex;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

static CAPABILITY_LEDGER_LOCK: Mutex<()> = Mutex::new(());

#[derive(Default, Deserialize, Serialize)]
struct CapabilityLedger {
    capabilities: BTreeMap<String, String>,
}

pub(crate) fn issue(path: &Path, notification_id: &str) -> Result<String> {
    let _guard = CAPABILITY_LEDGER_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut ledger = load(path);
    let capability = Uuid::new_v4().simple().to_string();
    ledger
        .capabilities
        .insert(notification_id.to_string(), capability.clone());
    save(path, &ledger)?;
    Ok(capability)
}

/// Verify and consume a capability in one serialized operation.  Consuming it
/// prevents a captured protocol URI from replaying a destructive action.
pub(crate) fn consume(path: &Path, notification_id: &str, provided: &str) -> Result<bool> {
    let _guard = CAPABILITY_LEDGER_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut ledger = load(path);
    let valid = ledger
        .capabilities
        .get(notification_id)
        .is_some_and(|expected| constant_time_eq(expected.as_bytes(), provided.as_bytes()));
    if valid {
        ledger.capabilities.remove(notification_id);
        save(path, &ledger)?;
    }
    Ok(valid)
}

pub(crate) fn revoke(path: &Path, notification_ids: &[String]) -> Result<()> {
    let _guard = CAPABILITY_LEDGER_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut ledger = load(path);
    let before = ledger.capabilities.len();
    for id in notification_ids {
        ledger.capabilities.remove(id);
    }
    if ledger.capabilities.len() != before {
        save(path, &ledger)?;
    }
    Ok(())
}

pub(crate) fn revoke_all(path: &Path) -> Result<()> {
    let _guard = CAPABILITY_LEDGER_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    save(path, &CapabilityLedger::default())
}

fn load(path: &Path) -> CapabilityLedger {
    fs::read(path)
        .ok()
        .and_then(|raw| serde_json::from_slice(&raw).ok())
        .unwrap_or_default()
}

fn save(path: &Path, ledger: &CapabilityLedger) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create capability directory {}", parent.display()))?;
    }
    let raw = serde_json::to_vec(ledger).context("serialize Windows notification capabilities")?;
    fs::write(path, raw)
        .with_context(|| format!("write notification capabilities {}", path.display()))
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut different = left.len() ^ right.len();
    let length = left.len().max(right.len());
    for index in 0..length {
        different |= usize::from(
            left.get(index).copied().unwrap_or_default()
                ^ right.get(index).copied().unwrap_or_default(),
        );
    }
    different == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ledger_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "knotq-windows-capability-{name}-{}-{}.json",
            std::process::id(),
            Uuid::new_v4()
        ))
    }

    #[test]
    fn capability_is_required_and_consumed_once() {
        let path = ledger_path("consume");
        let capability = issue(&path, "notification").unwrap();

        assert!(!consume(&path, "notification", "forged").unwrap());
        assert!(consume(&path, "notification", &capability).unwrap());
        assert!(!consume(&path, "notification", &capability).unwrap());

        let _ = fs::remove_file(path);
    }

    #[test]
    fn reissuing_invalidates_the_previous_capability() {
        let path = ledger_path("reissue");
        let old = issue(&path, "notification").unwrap();
        let current = issue(&path, "notification").unwrap();

        assert!(!consume(&path, "notification", &old).unwrap());
        assert!(consume(&path, "notification", &current).unwrap());

        let _ = fs::remove_file(path);
    }

    #[test]
    fn cancellation_revokes_only_targeted_capabilities() {
        let path = ledger_path("revoke");
        let first = issue(&path, "first").unwrap();
        let second = issue(&path, "second").unwrap();
        revoke(&path, &["first".to_string()]).unwrap();

        assert!(!consume(&path, "first", &first).unwrap());
        assert!(consume(&path, "second", &second).unwrap());

        let _ = fs::remove_file(path);
    }

    #[test]
    fn cancel_all_revokes_every_capability() {
        let path = ledger_path("revoke-all");
        let first = issue(&path, "first").unwrap();
        let second = issue(&path, "second").unwrap();
        revoke_all(&path).unwrap();

        assert!(!consume(&path, "first", &first).unwrap());
        assert!(!consume(&path, "second", &second).unwrap());

        let _ = fs::remove_file(path);
    }
}
