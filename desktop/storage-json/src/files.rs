use anyhow::{anyhow, Context, Result};
use chrono::NaiveDate;
use knotq_model::{Scheme, SchemeId, Workspace};
use std::collections::HashSet;
use std::io;
use std::sync::Mutex;
use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::{
    cal_index::daily_queue_calendar_index_matches_range,
    options::WorkspaceLoadOptions,
    schema::{WorkspaceEnvelope, WorkspaceIndex},
    scheme_file::{
        ensure_scheme_directories, prune_removed_scheme_files, read_daily_queue_file,
        read_existing_daily_queue_index, write_daily_backup, write_scheme_file,
    },
};

pub(crate) const SCHEMA_VERSION: u32 = 1;
pub(crate) const SETTINGS_SCHEMA_VERSION: u32 = 1;

/// Serializes whole-workspace saves. The debounced save task and the sync-run
/// save both call into here from background threads; without this, their
/// scheme-file/index write sets interleave and `prune_removed_scheme_files`
/// can act on a half-written sibling snapshot.
static WORKSPACE_SAVE_LOCK: Mutex<()> = Mutex::new(());

fn lock_workspace_save() -> std::sync::MutexGuard<'static, ()> {
    WORKSPACE_SAVE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
const WORKSPACE_GITIGNORE: &str =
    "# KnotQ local files\n.knotq-history/\nbackups/\n*.tmp\n.DS_Store\n";

pub fn load_workspace(path: &Path) -> Result<Option<Workspace>> {
    load_workspace_with_options(path, WorkspaceLoadOptions::all())
}

pub fn load_workspace_with_options(
    path: &Path,
    options: WorkspaceLoadOptions,
) -> Result<Option<Workspace>> {
    let Some(env) = read_workspace_envelope(path)? else {
        return Ok(None);
    };
    let base_dir = path.parent().unwrap_or_else(|| Path::new("."));
    env.workspace
        .into_workspace_with_options(base_dir, options)
        .map(Some)
}

pub fn load_daily_queue_scheme(path: &Path, date: NaiveDate) -> Result<Option<Scheme>> {
    let Some(env) = read_workspace_envelope(path)? else {
        return Ok(None);
    };
    let Some(entry) = env
        .workspace
        .daily_queue
        .into_iter()
        .find(|entry| entry.date == date)
    else {
        return Ok(None);
    };
    let base_dir = path.parent().unwrap_or_else(|| Path::new("."));
    let file = match read_daily_queue_file(base_dir, date, entry.scheme.id) {
        Ok(file) => file,
        Err(err) if is_not_found(&err) => return Ok(None),
        Err(err) => return Err(err),
    };
    if file.id != entry.scheme.id {
        return Err(anyhow!(
            "daily queue scheme {} contains id {}",
            entry.scheme.id,
            file.id
        ));
    }
    Ok(Some(crate::scheme_file::scheme_from_index(
        entry.scheme,
        file.items,
    )))
}

pub fn load_daily_queue_schemes_for_calendar_range(
    path: &Path,
    start: NaiveDate,
    end: NaiveDate,
) -> Result<Vec<(NaiveDate, Scheme)>> {
    let Some(env) = read_workspace_envelope(path)? else {
        return Ok(Vec::new());
    };
    let base_dir = path.parent().unwrap_or_else(|| Path::new("."));
    let mut schemes = Vec::new();
    for entry in env.workspace.daily_queue {
        if !daily_queue_calendar_index_matches_range(
            &entry.scheme.calendar_index,
            Some(start),
            Some(end),
        ) {
            continue;
        }
        let file = match read_daily_queue_file(base_dir, entry.date, entry.scheme.id) {
            Ok(file) => file,
            Err(err) => {
                if is_not_found(&err) {
                    continue;
                }
                return Err(err);
            }
        };
        if file.id != entry.scheme.id {
            return Err(anyhow!(
                "daily queue scheme {} contains id {}",
                entry.scheme.id,
                file.id
            ));
        }
        schemes.push((
            entry.date,
            crate::scheme_file::scheme_from_index(entry.scheme, file.items),
        ));
    }
    Ok(schemes)
}

pub fn save_workspace(path: &Path, workspace: &Workspace) -> Result<()> {
    let _guard = lock_workspace_save();
    let (base_dir, workspace) = prepare_workspace_save(path, workspace)?;
    for scheme in workspace.schemes.values() {
        write_scheme_file(&base_dir, &workspace, scheme)
            .with_context(|| format!("write scheme {}", scheme.id))?;
    }
    prune_removed_scheme_files(&base_dir, &workspace)?;

    let json = write_workspace_index(path, &workspace)?;
    write_daily_backup(&base_dir, &json, &workspace);
    record_history_snapshot(&base_dir);

    Ok(())
}

/// Save only the specified dirty schemes and the workspace index.
/// Skips the daily backup for speed; full saves also rewrite every scheme file.
pub fn save_workspace_incremental(
    path: &Path,
    workspace: &Workspace,
    dirty_scheme_ids: &HashSet<SchemeId>,
) -> Result<()> {
    let _guard = lock_workspace_save();
    let (base_dir, workspace) = prepare_workspace_save(path, workspace)?;
    // Write only dirty scheme files.
    for scheme_id in dirty_scheme_ids {
        if let Some(scheme) = workspace.schemes.get(scheme_id) {
            write_scheme_file(&base_dir, &workspace, scheme)
                .with_context(|| format!("write scheme {}", scheme.id))?;
        }
    }
    prune_removed_scheme_files(&base_dir, &workspace)?;

    // Always rewrite the workspace index (it's small and metadata may have changed).
    write_workspace_index(path, &workspace)?;
    record_history_snapshot(&base_dir);

    Ok(())
}

fn prepare_workspace_save(path: &Path, workspace: &Workspace) -> Result<(PathBuf, Workspace)> {
    let mut workspace = workspace.clone();
    workspace.ensure_sync_metadata();
    let base_dir = path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    ensure_workspace_layout(&base_dir, &workspace)?;
    Ok((base_dir, workspace))
}

fn ensure_workspace_layout(base_dir: &Path, workspace: &Workspace) -> Result<()> {
    fs::create_dir_all(base_dir).with_context(|| format!("create {}", base_dir.display()))?;
    ensure_workspace_gitignore(base_dir)?;
    let schemes_dir = base_dir.join("schemes");
    fs::create_dir_all(&schemes_dir)
        .with_context(|| format!("create {}", schemes_dir.display()))?;
    ensure_scheme_directories(base_dir, workspace)
}

fn write_workspace_index(path: &Path, workspace: &Workspace) -> Result<String> {
    let existing_daily_queue = read_existing_daily_queue_index(path)?;
    let env = WorkspaceEnvelope {
        version: SCHEMA_VERSION,
        workspace: WorkspaceIndex::from_workspace_preserving(workspace, existing_daily_queue),
    };
    let json = serde_json::to_string_pretty(&env)?;
    write_atomic(path, json.as_bytes())?;
    Ok(json)
}

pub(crate) fn read_workspace_envelope(path: &Path) -> Result<Option<WorkspaceEnvelope>> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    if raw.trim().is_empty() {
        return Ok(None);
    }
    let env = parse_workspace_envelope(&raw, path)?;
    validate_workspace_version(env.version)?;
    Ok(Some(env))
}

/// Parse the workspace index, recovering from trailing-garbage corruption.
///
/// Builds before the unique-tmp-name fix in `write_atomic` could publish an
/// index consisting of a complete document followed by the tail of the
/// previous, longer version. The prefix is a complete recent snapshot, so
/// salvage it (the next save rewrites the file cleanly) instead of wedging
/// every subsequent save and sync run behind the parse error.
pub(crate) fn parse_workspace_envelope(raw: &str, path: &Path) -> Result<WorkspaceEnvelope> {
    let err = match serde_json::from_str(raw) {
        Ok(env) => return Ok(env),
        Err(err) => err,
    };
    let mut stream = serde_json::Deserializer::from_str(raw).into_iter::<WorkspaceEnvelope>();
    if let Some(Ok(env)) = stream.next() {
        eprintln!(
            "recovered workspace index {} from trailing data after the document (was: {err})",
            path.display()
        );
        return Ok(env);
    }
    Err(err).context("parse workspace index")
}

pub(crate) fn validate_workspace_version(version: u32) -> Result<()> {
    if !(1..=SCHEMA_VERSION).contains(&version) {
        return Err(anyhow!(
            "unsupported workspace schema version {}, expected 1..={}",
            version,
            SCHEMA_VERSION
        ));
    }
    Ok(())
}

pub(crate) fn write_atomic(path: &Path, contents: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    }
    // The tmp name must be unique per call: concurrent writers to the same
    // path (the debounced save task racing a sync-run save) sharing one tmp
    // file interleave their writes, publishing the shorter document with the
    // longer one's tail appended — a workspace index that no longer parses.
    let unique = format!(
        "{}-{}",
        std::process::id(),
        TMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    let tmp = match path.extension().and_then(|ext| ext.to_str()) {
        Some(ext) => path.with_extension(format!("{ext}.{unique}.tmp")),
        None => path.with_extension(format!("{unique}.tmp")),
    };
    let write_result = (|| {
        let mut file =
            fs::File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
        file.write_all(contents)
            .with_context(|| format!("write {}", tmp.display()))?;
        // Flush to disk before the rename publishes the file: without this a
        // crash or I/O stall can land the rename ahead of the data and leave
        // a zero-length "complete" file behind.
        file.sync_all()
            .with_context(|| format!("sync {}", tmp.display()))?;
        fs::rename(&tmp, path).with_context(|| format!("rename {}", path.display()))
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    write_result
}

fn ensure_workspace_gitignore(base_dir: &Path) -> Result<()> {
    let path = base_dir.join(".gitignore");
    if !path.exists() {
        return write_atomic(&path, WORKSPACE_GITIGNORE.as_bytes());
    }
    let existing = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let mut updated = existing.clone();
    for line in [".knotq-history/", "backups/", "*.tmp", ".DS_Store"] {
        if !existing.lines().any(|existing_line| existing_line == line) {
            if !updated.ends_with('\n') {
                updated.push('\n');
            }
            updated.push_str(line);
            updated.push('\n');
        }
    }
    if updated == existing {
        return Ok(());
    }
    write_atomic(&path, updated.as_bytes())
}

fn record_history_snapshot(base_dir: &Path) {
    if let Err(err) = knotq_history::record_workspace_snapshot(base_dir) {
        eprintln!("workspace history snapshot failed: {err:#}");
    }
}

fn is_not_found(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<io::Error>()
            .is_some_and(|err| err.kind() == io::ErrorKind::NotFound)
    })
}

pub(crate) fn is_false(value: &bool) -> bool {
    !*value
}

#[cfg(test)]
mod tests {
    use super::*;

    // clippy::redundant_iter_cloned false positive: the suggested fix drops
    // `.cloned()` and moves `&Vec<u8>` borrows into `thread::spawn`, which
    // needs `'static` and does not compile (verified).
    #[allow(clippy::redundant_iter_cloned)]
    #[test]
    fn concurrent_write_atomic_always_publishes_one_complete_document() {
        let dir =
            std::env::temp_dir().join(format!("knotq-write-atomic-race-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("target.json");

        // One short and one long payload: with a shared tmp file the
        // interleaving publishes the short payload with the long one's tail.
        let short = vec![b'a'; 64];
        let long = vec![b'b'; 512 * 1024];
        let handles: Vec<_> = [&short, &long, &short, &long]
            .into_iter()
            .cloned()
            .map(|contents| {
                let path = path.clone();
                std::thread::spawn(move || {
                    for _ in 0..25 {
                        write_atomic(&path, &contents).unwrap();
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }

        let published = fs::read(&path).unwrap();
        assert!(
            published == short || published == long,
            "published file must be exactly one writer's payload, got {} bytes",
            published.len()
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
