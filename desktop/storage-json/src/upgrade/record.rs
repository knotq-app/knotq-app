//! What has already been done to this data directory, and what was in flight.
//!
//! Two files, both in the data directory root:
//!
//! - `data-layout.json` — the layout version this build family writes and the
//!   ids of the migrations that have completed. Read at startup, written after.
//! - `data-upgrade-journal.json` — the migrations currently being applied.
//!   Present at startup only if a previous run was killed part-way through.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// The shape of the data directory as a whole: bumped when a migration is added.
///
/// This is *not* the schema version of any single file (`workspace.json` and
/// `settings.json` carry their own). It answers a different question: has this
/// directory been through the migrations this build expects, and was it last
/// touched by a build newer than this one?
pub const DATA_LAYOUT_VERSION: u32 = 1;

const LAYOUT_FILE: &str = "data-layout.json";
const JOURNAL_FILE: &str = "data-upgrade-journal.json";

pub fn data_layout_path(data_dir: &Path) -> PathBuf {
    data_dir.join(LAYOUT_FILE)
}

fn journal_path(data_dir: &Path) -> PathBuf {
    data_dir.join(JOURNAL_FILE)
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct DataLayoutRecord {
    /// Zero for a directory from before this module existed.
    #[serde(default)]
    pub layout_version: u32,
    /// Ids of completed migrations, in the order they ran.
    #[serde(default)]
    pub applied: Vec<String>,
    /// Version of the crate that last wrote this file, for support logs.
    #[serde(default)]
    pub last_written_by: String,
    #[serde(default)]
    pub updated_at: Option<DateTime<Utc>>,
}

impl DataLayoutRecord {
    pub fn mark_applied(&mut self, id: &str) {
        if !self.applied.iter().any(|applied| applied == id) {
            self.applied.push(id.to_string());
        }
    }

    pub fn has_applied(&self, id: &str) -> bool {
        self.applied.iter().any(|applied| applied == id)
    }
}

pub fn load(data_dir: &Path) -> Result<DataLayoutRecord> {
    let path = data_layout_path(data_dir);
    if !path.exists() {
        return Ok(DataLayoutRecord::default());
    }
    let raw = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    if raw.trim().is_empty() {
        return Ok(DataLayoutRecord::default());
    }
    serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))
}

pub fn save(data_dir: &Path, record: &DataLayoutRecord) -> Result<()> {
    let mut record = DataLayoutRecord {
        layout_version: record.layout_version,
        applied: record.applied.clone(),
        last_written_by: record.last_written_by.clone(),
        updated_at: Some(Utc::now()),
    };
    record.applied.dedup();
    let json = serde_json::to_string_pretty(&record)?;
    crate::files::write_atomic(&data_layout_path(data_dir), json.as_bytes())
}

/// Record that `id` is about to be applied. Durable before the change itself, so
/// a crash during `apply` is visible on the next launch.
pub fn begin(data_dir: &Path, id: &str) -> Result<()> {
    let mut in_flight = read_journal(data_dir);
    if !in_flight.iter().any(|entry| entry == id) {
        in_flight.push(id.to_string());
    }
    write_journal(data_dir, &in_flight)
}

/// Clear `id` from the journal. Best effort: the migration is already done and
/// verified, and a leftover entry only costs an idempotent re-run.
pub fn finish(data_dir: &Path, id: &str) {
    let in_flight: Vec<String> = read_journal(data_dir)
        .into_iter()
        .filter(|entry| entry != id)
        .collect();
    let _ = write_journal(data_dir, &in_flight);
}

/// The migrations a previous run began and never finished. Read once at startup;
/// the entries stay in the journal until their re-run finishes.
pub fn take_unfinished(data_dir: &Path) -> Vec<String> {
    read_journal(data_dir)
}

fn read_journal(data_dir: &Path) -> Vec<String> {
    let path = journal_path(data_dir);
    let Ok(raw) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

fn write_journal(data_dir: &Path, in_flight: &[String]) -> Result<()> {
    let path = journal_path(data_dir);
    if in_flight.is_empty() {
        if path.exists() {
            fs::remove_file(&path).with_context(|| format!("remove {}", path.display()))?;
        }
        return Ok(());
    }
    let json = serde_json::to_string(in_flight)?;
    crate::files::write_atomic(&path, json.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "knotq-layout-{name}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn an_absent_record_reads_as_an_unmigrated_directory() {
        let dir = temp_dir("absent");
        let record = load(&dir).unwrap();
        assert_eq!(record.layout_version, 0);
        assert!(record.applied.is_empty());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn the_journal_survives_until_the_migration_finishes() {
        let dir = temp_dir("journal");
        begin(&dir, "one").unwrap();
        begin(&dir, "two").unwrap();
        assert_eq!(take_unfinished(&dir), vec!["one", "two"]);

        finish(&dir, "one");
        assert_eq!(take_unfinished(&dir), vec!["two"]);

        finish(&dir, "two");
        assert!(take_unfinished(&dir).is_empty());
        assert!(!journal_path(&dir).exists(), "an empty journal is removed");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn applied_ids_round_trip() {
        let dir = temp_dir("applied");
        let mut record = DataLayoutRecord::default();
        record.mark_applied("one");
        record.mark_applied("one");
        record.layout_version = DATA_LAYOUT_VERSION;
        save(&dir, &record).unwrap();

        let loaded = load(&dir).unwrap();
        assert_eq!(loaded.applied, vec!["one"]);
        assert_eq!(loaded.layout_version, DATA_LAYOUT_VERSION);
        assert!(loaded.has_applied("one"));
        assert!(loaded.updated_at.is_some());
        let _ = fs::remove_dir_all(dir);
    }
}
