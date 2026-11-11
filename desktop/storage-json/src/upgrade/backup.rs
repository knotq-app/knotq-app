//! The copy taken before a migration runs, and put back if it fails.
//!
//! Kept deliberately dumb — a plain recursive file copy under
//! `upgrade-backups/<id>-<timestamp>/`, mirroring the paths relative to the data
//! directory. It has to be readable by a human on a support call and restorable
//! by hand, so nothing is packed, compressed or renamed.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

const BACKUP_ROOT: &str = "upgrade-backups";
/// How many upgrade backups to keep. They are small (a migration declares a
/// bounded path set) but they are dead weight once a build has run for a while.
const KEEP: usize = 3;
/// A backup older than this is dropped even if it is among the newest
/// [`KEEP`]: a migration that has run cleanly for four weeks has stuck.
const MAX_AGE_DAYS: i64 = 28;

pub struct Snapshot {
    pub dir: PathBuf,
    /// Absolute originals, paired with their copy. An original that did not
    /// exist has `None` — restoring it means removing whatever is there now.
    entries: Vec<(PathBuf, Option<PathBuf>)>,
}

pub fn create(data_dir: &Path, id: &str, targets: &[PathBuf]) -> Result<Snapshot> {
    let dir = data_dir.join(BACKUP_ROOT).join(format!(
        "{id}-{}",
        chrono::Utc::now().format("%Y%m%dT%H%M%S%.3f")
    ));
    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;

    let mut entries = Vec::with_capacity(targets.len());
    for target in targets {
        if !target.exists() {
            // Recorded anyway: restoring must be able to delete a file the
            // migration created where there was none.
            entries.push((target.clone(), None));
            continue;
        }
        let relative = target.strip_prefix(data_dir).unwrap_or(target.as_path());
        let copy = dir.join(relative);
        copy_recursive(target, &copy)
            .with_context(|| format!("copy {} for backup", target.display()))?;
        entries.push((target.clone(), Some(copy)));
    }
    Ok(Snapshot { dir, entries })
}

pub fn restore(snapshot: &Snapshot) -> Result<()> {
    for (original, copy) in &snapshot.entries {
        remove_any(original)
            .with_context(|| format!("clear {} before restoring", original.display()))?;
        let Some(copy) = copy else { continue };
        copy_recursive(copy, original)
            .with_context(|| format!("restore {}", original.display()))?;
    }
    Ok(())
}

/// Drop all but the newest [`KEEP`] backups, and any older than
/// [`MAX_AGE_DAYS`].
pub fn prune(data_dir: &Path) {
    prune_at(data_dir, chrono::Utc::now());
}

fn prune_at(data_dir: &Path, now: chrono::DateTime<chrono::Utc>) {
    let root = data_dir.join(BACKUP_ROOT);
    let Ok(entries) = fs::read_dir(&root) else {
        return;
    };
    // The timestamp is in the name and fixed-width, so sorting by name is
    // chronological and does not depend on mtimes surviving a copy or restore.
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    let cutoff = now - chrono::Duration::days(MAX_AGE_DAYS);
    let excess = dirs.len().saturating_sub(KEEP);
    for (index, dir) in dirs.iter().enumerate() {
        // An unparseable name is never aged out, only rotated out.
        let expired = backup_time(dir).is_some_and(|taken| taken < cutoff);
        if index < excess || expired {
            let _ = fs::remove_dir_all(dir);
        }
    }
}

/// The timestamp `create` put at the end of the directory name.
fn backup_time(dir: &Path) -> Option<chrono::DateTime<chrono::Utc>> {
    let name = dir.file_name()?.to_str()?;
    let (_, stamp) = name.rsplit_once('-')?;
    chrono::NaiveDateTime::parse_from_str(stamp, "%Y%m%dT%H%M%S%.3f")
        .ok()
        .map(|naive| naive.and_utc())
}

fn copy_recursive(from: &Path, to: &Path) -> Result<()> {
    if from.is_dir() {
        fs::create_dir_all(to).with_context(|| format!("create {}", to.display()))?;
        for entry in fs::read_dir(from).with_context(|| format!("read {}", from.display()))? {
            let entry = entry?;
            copy_recursive(&entry.path(), &to.join(entry.file_name()))?;
        }
        return Ok(());
    }
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    fs::copy(from, to).with_context(|| format!("copy {} to {}", from.display(), to.display()))?;
    Ok(())
}

fn remove_any(path: &Path) -> Result<()> {
    if path.is_dir() {
        fs::remove_dir_all(path)?;
    } else if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "knotq-upgrade-backup-{name}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn restoring_puts_files_directories_and_absences_back() {
        let data_dir = temp_dir("restore");
        let file = data_dir.join("state.json");
        let tree = data_dir.join("documents");
        let created_by_migration = data_dir.join("new-form");
        fs::write(&file, b"original").unwrap();
        fs::create_dir_all(tree.join("nested")).unwrap();
        fs::write(tree.join("nested").join("a.ydoc"), b"aaa").unwrap();

        let snapshot = create(
            &data_dir,
            "test",
            &[file.clone(), tree.clone(), created_by_migration.clone()],
        )
        .unwrap();

        // A migration that mangles all three.
        fs::write(&file, b"half written").unwrap();
        fs::remove_dir_all(&tree).unwrap();
        fs::create_dir_all(&created_by_migration).unwrap();
        fs::write(created_by_migration.join("junk"), b"x").unwrap();

        restore(&snapshot).unwrap();

        assert_eq!(fs::read(&file).unwrap(), b"original");
        assert_eq!(
            fs::read(tree.join("nested").join("a.ydoc")).unwrap(),
            b"aaa"
        );
        assert!(
            !created_by_migration.exists(),
            "what the migration created must be gone again"
        );
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn only_the_newest_backups_are_kept() {
        let data_dir = temp_dir("prune");
        let file = data_dir.join("state.json");
        fs::write(&file, b"x").unwrap();
        let mut made = Vec::new();
        for _ in 0..KEEP + 2 {
            made.push(
                create(&data_dir, "test", std::slice::from_ref(&file))
                    .unwrap()
                    .dir,
            );
            // The name carries a millisecond timestamp; keep them distinct.
            std::thread::sleep(std::time::Duration::from_millis(2));
        }

        prune(&data_dir);

        let kept: Vec<PathBuf> = fs::read_dir(data_dir.join(BACKUP_ROOT))
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .collect();
        assert_eq!(kept.len(), KEEP);
        for newest in &made[made.len() - KEEP..] {
            assert!(kept.contains(newest), "{} was pruned", newest.display());
        }
        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn backups_older_than_four_weeks_are_dropped() {
        let data_dir = temp_dir("prune-age");
        let file = data_dir.join("state.json");
        fs::write(&file, b"x").unwrap();
        let made = create(&data_dir, "crdt-state-per-document-files", &[file])
            .unwrap()
            .dir;
        assert!(
            backup_time(&made).is_some(),
            "name must carry its timestamp"
        );

        prune_at(
            &data_dir,
            chrono::Utc::now() + chrono::Duration::days(MAX_AGE_DAYS - 1),
        );
        assert!(made.exists(), "a recent backup must survive");

        prune_at(
            &data_dir,
            chrono::Utc::now() + chrono::Duration::days(MAX_AGE_DAYS + 1),
        );
        assert!(!made.exists(), "an expired backup must be dropped");
        let _ = fs::remove_dir_all(data_dir);
    }
}
