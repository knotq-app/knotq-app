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
        read_existing_daily_queue_index, scheme_path_for_workspace, write_daily_backup,
        write_scheme_file,
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

/// Whether to print the per-phase cost of a save (`KNOTQ_EDIT_TIMING=1`).
/// Read once: this sits on the path taken by every edit.
pub fn edit_timing_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("KNOTQ_EDIT_TIMING").is_some())
}

pub fn save_workspace(path: &Path, workspace: &Workspace) -> Result<()> {
    let timing = edit_timing_enabled();
    let t0 = std::time::Instant::now();
    let _guard = lock_workspace_save();
    let (base_dir, workspace) = prepare_workspace_save(path, workspace)?;
    let t1 = std::time::Instant::now();
    for scheme in workspace.schemes.values() {
        write_scheme_file(&base_dir, &workspace, scheme)
            .with_context(|| format!("write scheme {}", scheme.id))?;
    }
    prune_removed_scheme_files(&base_dir, &workspace)?;
    let t2 = std::time::Instant::now();

    let json = write_workspace_index(path, &workspace)?;
    let t3 = std::time::Instant::now();
    write_daily_backup(&base_dir, &json, &workspace);
    let t4 = std::time::Instant::now();
    record_history_snapshot(&base_dir);
    if timing {
        eprintln!(
            "  save_workspace: prepare {}ms, {} scheme files {}ms, index {}ms, daily backup {}ms, history {}ms",
            (t1 - t0).as_millis(),
            workspace.schemes.len(),
            (t2 - t1).as_millis(),
            (t3 - t2).as_millis(),
            (t4 - t3).as_millis(),
            t4.elapsed().as_millis()
        );
    }

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
    // A lazily loaded or starter scheme may not be in the dirty set when an
    // incremental save first creates the workspace index. Never leave any
    // indexed scheme without its body: the next launch treats that as a
    // damaged workspace and moves the whole directory aside. Existing files
    // remain untouched, so this repair is limited to missing files.
    for (scheme_id, scheme) in &workspace.schemes {
        let Some(path) = scheme_path_for_workspace(&base_dir, &workspace, *scheme_id)? else {
            continue;
        };
        if !path.exists() {
            write_scheme_file(&base_dir, &workspace, scheme)
                .with_context(|| format!("write missing daily scheme {}", scheme.id))?;
        }
    }
    prune_removed_scheme_files(&base_dir, &workspace)?;

    // Always rewrite the workspace index (it's small and metadata may have changed).
    write_workspace_index(path, &workspace)?;
    record_history_snapshot(&base_dir);

    Ok(())
}

/// Write the plain files of schemes the saved workspace does not hold.
///
/// The data directory has two halves: the plain files the app reads, and the
/// CRDT document states sync merges into. A save that writes one half without
/// the other leaves the directory describing two different workspaces, and the
/// next sync reads that difference as a local edit — which is how a Daily page
/// outside the loaded window got its own merged content re-asserted back to the
/// stale copy on disk.
///
/// `workspace` supplies the index (so a Daily page still resolves to its
/// `daily_queue/YYYY/MM/DD.knotq` path even though its body is not loaded);
/// `schemes` supplies the bodies, materialized from the documents being
/// written. Only files are touched — nothing is pruned and the index is not
/// rewritten, because the caller's own save owns both.
pub fn save_unloaded_scheme_files(
    path: &Path,
    workspace: &Workspace,
    schemes: &[Scheme],
) -> Result<()> {
    if schemes.is_empty() {
        return Ok(());
    }
    let _guard = lock_workspace_save();
    let (base_dir, workspace) = prepare_workspace_save(path, workspace)?;
    for scheme in schemes {
        if workspace.schemes.contains_key(&scheme.id) {
            // The ordinary save already owns this one.
            continue;
        }
        if scheme_path_for_workspace(&base_dir, &workspace, scheme.id)?.is_none() {
            // Not addressable from this index (no folder placement and no daily
            // binding): writing it would put a file where nothing looks.
            continue;
        }
        write_scheme_file(&base_dir, &workspace, scheme)
            .with_context(|| format!("write unloaded scheme {}", scheme.id))?;
    }
    refresh_unloaded_daily_index_entries(path, schemes)
}

/// Bring the workspace-index entries of unloaded Daily pages up to date.
///
/// A Daily page's name and colour live in `workspace.json`, not in its own
/// file, and the index write preserves the stored entry for any page whose body
/// is not loaded (`WorkspaceIndex::from_workspace_preserving`) — it has nothing
/// better to write it from. So a remote rename or recolour of an off-window day
/// reached the CRDT and stopped there: the next time the page entered the
/// window it was read back from disk with its old colour, and the device's two
/// halves disagreed from then on (production fuzz single-account seed 10105).
///
/// These schemes are materialized from this device's own documents, so they are
/// exactly what the index should say. The `calendar_index` is left alone: it is
/// derived from the items, which the body write above owns.
fn refresh_unloaded_daily_index_entries(path: &Path, schemes: &[Scheme]) -> Result<()> {
    let by_id: std::collections::HashMap<SchemeId, &Scheme> =
        schemes.iter().map(|s| (s.id, s)).collect();
    let Some(mut env) = read_workspace_envelope(path)? else {
        return Ok(());
    };
    let mut changed = false;
    for entry in &mut env.workspace.daily_queue {
        let Some(scheme) = by_id.get(&entry.scheme.id) else {
            continue;
        };
        if entry.scheme.name == scheme.name
            && entry.scheme.color_index == scheme.color_index
            && entry.scheme.gsync == scheme.gsync
            && entry.scheme.source == scheme.source
        {
            continue;
        }
        entry.scheme.name = scheme.name.clone();
        entry.scheme.color_index = scheme.color_index;
        entry.scheme.gsync = scheme.gsync;
        entry.scheme.source = scheme.source.clone();
        changed = true;
    }
    if !changed {
        return Ok(());
    }
    let json = serde_json::to_string_pretty(&env)?;
    write_atomic_if_changed(path, json.as_bytes())?;
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
    // An edit to a scheme's items usually leaves the index (names, colours,
    // folder tree) untouched. The callers still get the JSON back — the daily
    // backup is written from it — but unchanged bytes need no durable rewrite.
    write_atomic_if_changed(path, json.as_bytes())?;
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
        fs::rename(&tmp, path).with_context(|| format!("rename {}", path.display()))?;
        // `sync_all` above makes the replacement file durable, but a rename is
        // a directory operation. Flush the containing directory too so a power
        // loss cannot leave the old name (or neither name) after callers were
        // told the atomic save succeeded. `std` cannot do the corresponding
        // directory flush on Windows; its rename semantics remain the platform
        // default there, while every Unix target we ship (macOS, Linux, iOS)
        // gets the stronger guarantee.
        sync_parent_directory(path)?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    write_result
}

/// Persist `contents` only when it differs from the current file.
///
/// Full workspace saves visit every scheme and the workspace index, although a
/// typical edit changes only one scheme. Keeping this fast path next to the
/// atomic writer makes it consistent for both kinds of file.
pub(crate) fn write_atomic_if_changed(path: &Path, contents: &[u8]) -> Result<()> {
    if fs::read(path).is_ok_and(|existing| existing == contents) {
        return Ok(());
    }
    write_atomic(path, contents)
}

#[cfg(unix)]
fn sync_parent_directory(path: &Path) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::File::open(parent)
        .with_context(|| format!("open directory {}", parent.display()))?
        .sync_all()
        .with_context(|| format!("sync directory {}", parent.display()))
}

#[cfg(not(unix))]
fn sync_parent_directory(_path: &Path) -> Result<()> {
    Ok(())
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
    // Mobile has no history UI/API. Do not make its sync path scan and hash the
    // entire workspace (or surface dangling desktop-history refs in the mobile
    // log); desktop remains the authoritative history consumer and keeps the
    // synchronous capture behavior below.
    #[cfg(any(target_os = "android", target_os = "ios"))]
    let _ = base_dir;

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
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
