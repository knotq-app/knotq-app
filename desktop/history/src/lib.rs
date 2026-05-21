use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use std::{
    collections::{BTreeMap, HashSet},
    ffi::OsStr,
    fs, io,
    path::{Path, PathBuf},
    process::Command,
};

const HISTORY_DIR: &str = ".knotq-history";
const SNAPSHOT_REF_PREFIX: &str = "refs/knotq/snapshots";
const TRACKED_PATHS: &[&str] = &[
    "workspace.json",
    ".gitignore",
    "schemes",
    "daily_queue",
    "assets",
];
const AUTHOR_NAME: &str = "KnotQ";
const AUTHOR_EMAIL: &str = "history@knotq.local";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceSnapshot {
    pub id: String,
    pub timestamp: DateTime<Utc>,
    pub label: String,
}

#[derive(Clone, Debug)]
struct InternalSnapshot {
    id: String,
    timestamp: DateTime<Utc>,
    refname: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RetentionBucket {
    tier: &'static str,
    start_epoch_secs: i64,
}

impl RetentionBucket {
    fn refname(&self) -> String {
        format!(
            "{SNAPSHOT_REF_PREFIX}/{}/{}",
            self.tier, self.start_epoch_secs
        )
    }
}

pub fn record_workspace_snapshot(workspace_dir: &Path) -> Result<()> {
    record_workspace_snapshot_at(workspace_dir, Utc::now())
}

pub fn list_workspace_snapshots(workspace_dir: &Path) -> Result<Vec<WorkspaceSnapshot>> {
    if !history_repo_exists(workspace_dir) {
        return Ok(Vec::new());
    }
    rotate_snapshots(workspace_dir, Utc::now())?;
    let mut snapshots = list_internal_snapshots(workspace_dir)?
        .into_iter()
        .map(|snapshot| WorkspaceSnapshot {
            id: snapshot.id,
            timestamp: snapshot.timestamp,
            label: format_snapshot_label(snapshot.timestamp),
        })
        .collect::<Vec<_>>();
    snapshots.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    snapshots.dedup_by(|a, b| a.id == b.id);
    Ok(snapshots)
}

pub fn restore_workspace_snapshot(workspace_dir: &Path, snapshot_id: &str) -> Result<()> {
    if !history_repo_exists(workspace_dir) {
        bail!("workspace history has not been initialized");
    }
    validate_snapshot_id(snapshot_id)?;
    run_git(
        workspace_dir,
        [
            OsStr::new("cat-file"),
            OsStr::new("-e"),
            OsStr::new(&format!("{snapshot_id}^{{commit}}")),
        ],
    )
    .with_context(|| format!("find history snapshot {snapshot_id}"))?;

    let paths = tree_paths(workspace_dir, snapshot_id)?;
    if !paths.iter().any(|path| path == "workspace.json") {
        bail!("history snapshot {snapshot_id} does not contain workspace.json");
    }

    remove_if_exists(&workspace_dir.join("workspace.json"))?;
    remove_if_exists(&workspace_dir.join(".gitignore"))?;
    remove_if_exists(&workspace_dir.join("schemes"))?;
    remove_if_exists(&workspace_dir.join("daily_queue"))?;
    remove_if_exists(&workspace_dir.join("assets"))?;

    let mut checkout_paths = Vec::new();
    if paths.iter().any(|path| path == "workspace.json") {
        checkout_paths.push("workspace.json");
    }
    if paths.iter().any(|path| path == ".gitignore") {
        checkout_paths.push(".gitignore");
    }
    if paths
        .iter()
        .any(|path| path == "schemes" || path.starts_with("schemes/"))
    {
        checkout_paths.push("schemes");
    }
    if paths
        .iter()
        .any(|path| path == "daily_queue" || path.starts_with("daily_queue/"))
    {
        checkout_paths.push("daily_queue");
    }
    if paths
        .iter()
        .any(|path| path == "assets" || path.starts_with("assets/"))
    {
        checkout_paths.push("assets");
    }

    let mut args = vec!["checkout", "--force", snapshot_id, "--"];
    args.extend(checkout_paths);
    run_git(workspace_dir, args)
        .with_context(|| format!("restore history snapshot {snapshot_id}"))?;
    Ok(())
}

fn record_workspace_snapshot_at(workspace_dir: &Path, now: DateTime<Utc>) -> Result<()> {
    ensure_history_repo(workspace_dir)?;
    stage_tracked_paths(workspace_dir)?;
    let tree = run_git(workspace_dir, ["write-tree"])?.trim().to_string();
    if tree.is_empty() {
        bail!("git write-tree returned an empty tree id");
    }
    if let Some(latest) = latest_snapshot(workspace_dir)? {
        let latest_tree = tree_for_commit(workspace_dir, &latest.id)?;
        if latest_tree == tree {
            return Ok(());
        }
    }
    let bucket = retention_bucket(now, now)
        .ok_or_else(|| anyhow!("new history snapshots must be within retention"))?;
    let commit = create_commit(workspace_dir, &tree, now)?;
    update_ref(workspace_dir, &bucket.refname(), &commit)?;
    rotate_snapshots(workspace_dir, now)?;
    let _ = run_git(workspace_dir, ["gc", "--auto"]);
    Ok(())
}

fn ensure_history_repo(workspace_dir: &Path) -> Result<()> {
    fs::create_dir_all(workspace_dir)
        .with_context(|| format!("create {}", workspace_dir.display()))?;
    if history_repo_exists(workspace_dir) {
        return Ok(());
    }
    let history_dir = history_dir(workspace_dir);
    fs::create_dir_all(&history_dir)
        .with_context(|| format!("create {}", history_dir.display()))?;
    let output = Command::new("git")
        .arg("init")
        .arg("--bare")
        .arg(&history_dir)
        .output()
        .with_context(|| "run git init --bare for workspace history")?;
    if !output.status.success() {
        bail!(
            "git init --bare failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

fn history_repo_exists(workspace_dir: &Path) -> bool {
    history_dir(workspace_dir).join("HEAD").exists()
}

fn history_dir(workspace_dir: &Path) -> PathBuf {
    workspace_dir.join(HISTORY_DIR)
}

fn stage_tracked_paths(workspace_dir: &Path) -> Result<()> {
    let mut pathspecs = Vec::new();
    for path in TRACKED_PATHS {
        if workspace_dir.join(path).exists() || path_has_tracked_entries(workspace_dir, path)? {
            pathspecs.push(*path);
        }
    }
    if pathspecs.is_empty() {
        bail!("workspace history has no files to snapshot");
    }
    let mut args = vec!["add", "-A", "--"];
    args.extend(pathspecs);
    run_git(workspace_dir, args).context("stage workspace files for history")?;
    Ok(())
}

fn path_has_tracked_entries(workspace_dir: &Path, path: &str) -> Result<bool> {
    Ok(!run_git(workspace_dir, ["ls-files", "--", path])?
        .trim()
        .is_empty())
}

fn create_commit(workspace_dir: &Path, tree: &str, timestamp: DateTime<Utc>) -> Result<String> {
    let date = timestamp.to_rfc3339_opts(SecondsFormat::Secs, true);
    let message = format!("KnotQ history {}", format_snapshot_label(timestamp));
    let output = git_command(workspace_dir)
        .args(["commit-tree", tree, "-m", &message])
        .env("GIT_AUTHOR_NAME", AUTHOR_NAME)
        .env("GIT_AUTHOR_EMAIL", AUTHOR_EMAIL)
        .env("GIT_AUTHOR_DATE", &date)
        .env("GIT_COMMITTER_NAME", AUTHOR_NAME)
        .env("GIT_COMMITTER_EMAIL", AUTHOR_EMAIL)
        .env("GIT_COMMITTER_DATE", &date)
        .output()
        .with_context(|| "create workspace history commit")?;
    if !output.status.success() {
        bail!(
            "git commit-tree failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let commit = String::from_utf8(output.stdout)
        .context("decode git commit-tree output")?
        .trim()
        .to_string();
    validate_snapshot_id(&commit)?;
    Ok(commit)
}

fn latest_snapshot(workspace_dir: &Path) -> Result<Option<InternalSnapshot>> {
    Ok(list_internal_snapshots(workspace_dir)?
        .into_iter()
        .max_by_key(|snapshot| snapshot.timestamp))
}

fn list_internal_snapshots(workspace_dir: &Path) -> Result<Vec<InternalSnapshot>> {
    if !history_repo_exists(workspace_dir) {
        return Ok(Vec::new());
    }
    let output = run_git(
        workspace_dir,
        [
            "for-each-ref",
            "--format=%(objectname)%09%(committerdate:iso-strict)%09%(refname)",
            SNAPSHOT_REF_PREFIX,
        ],
    )?;
    let mut snapshots = Vec::new();
    for line in output.lines().filter(|line| !line.trim().is_empty()) {
        let mut parts = line.splitn(3, '\t');
        let id = parts
            .next()
            .ok_or_else(|| anyhow!("missing history snapshot id"))?
            .to_string();
        let timestamp = parts
            .next()
            .ok_or_else(|| anyhow!("missing history snapshot timestamp"))?;
        let refname = parts
            .next()
            .ok_or_else(|| anyhow!("missing history snapshot ref"))?
            .to_string();
        let timestamp = DateTime::parse_from_rfc3339(timestamp)
            .with_context(|| format!("parse history snapshot timestamp {timestamp:?}"))?
            .with_timezone(&Utc);
        snapshots.push(InternalSnapshot {
            id,
            timestamp,
            refname,
        });
    }
    Ok(snapshots)
}

fn rotate_snapshots(workspace_dir: &Path, now: DateTime<Utc>) -> Result<()> {
    let snapshots = list_internal_snapshots(workspace_dir)?;
    let mut keep_by_ref = BTreeMap::<String, InternalSnapshot>::new();
    for snapshot in &snapshots {
        let Some(bucket) = retention_bucket(snapshot.timestamp, now) else {
            continue;
        };
        let refname = bucket.refname();
        let replace = keep_by_ref
            .get(&refname)
            .map(|existing| snapshot.timestamp > existing.timestamp)
            .unwrap_or(true);
        if replace {
            keep_by_ref.insert(refname, snapshot.clone());
        }
    }

    for (refname, snapshot) in &keep_by_ref {
        update_ref(workspace_dir, refname, &snapshot.id)?;
    }

    let keep_refs = keep_by_ref.keys().cloned().collect::<HashSet<_>>();
    for snapshot in snapshots {
        if !keep_refs.contains(&snapshot.refname) {
            delete_ref(workspace_dir, &snapshot.refname)?;
        }
    }
    Ok(())
}

fn retention_bucket(timestamp: DateTime<Utc>, now: DateTime<Utc>) -> Option<RetentionBucket> {
    let age = now.signed_duration_since(timestamp);
    let (tier, step_secs) = if age <= Duration::hours(1) {
        ("m5", 5 * 60)
    } else if age <= Duration::hours(48) {
        ("h1", 60 * 60)
    } else if age <= Duration::days(7) {
        ("h4", 4 * 60 * 60)
    } else if age <= Duration::days(365) {
        ("d1", 24 * 60 * 60)
    } else {
        return None;
    };
    Some(RetentionBucket {
        tier,
        start_epoch_secs: floor_epoch(timestamp.timestamp(), step_secs),
    })
}

fn floor_epoch(timestamp: i64, step_secs: i64) -> i64 {
    timestamp.div_euclid(step_secs) * step_secs
}

fn update_ref(workspace_dir: &Path, refname: &str, commit: &str) -> Result<()> {
    run_git(workspace_dir, ["update-ref", refname, commit])
        .with_context(|| format!("update history ref {refname}"))?;
    Ok(())
}

fn delete_ref(workspace_dir: &Path, refname: &str) -> Result<()> {
    run_git(workspace_dir, ["update-ref", "-d", refname])
        .with_context(|| format!("delete history ref {refname}"))?;
    Ok(())
}

fn tree_for_commit(workspace_dir: &Path, commit: &str) -> Result<String> {
    Ok(
        run_git(workspace_dir, ["rev-parse", &format!("{commit}^{{tree}}")])?
            .trim()
            .to_string(),
    )
}

fn tree_paths(workspace_dir: &Path, commit: &str) -> Result<Vec<String>> {
    Ok(
        run_git(workspace_dir, ["ls-tree", "-r", "--name-only", commit])?
            .lines()
            .map(ToOwned::to_owned)
            .collect(),
    )
}

fn git_command(workspace_dir: &Path) -> Command {
    let mut command = Command::new("git");
    command.arg(format!(
        "--git-dir={}",
        history_dir(workspace_dir).display()
    ));
    command.arg(format!("--work-tree={}", workspace_dir.display()));
    command
}

fn run_git<I, S>(workspace_dir: &Path, args: I) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = git_command(workspace_dir)
        .args(args)
        .output()
        .with_context(|| "run git for workspace history")?;
    if !output.status.success() {
        bail!(
            "git failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    String::from_utf8(output.stdout).context("decode git output")
}

fn remove_if_exists(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => {
            fs::remove_dir_all(path).with_context(|| format!("remove {}", path.display()))
        }
        Ok(_) => fs::remove_file(path).with_context(|| format!("remove {}", path.display())),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| format!("stat {}", path.display())),
    }
}

fn validate_snapshot_id(id: &str) -> Result<()> {
    if id.len() < 7 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("invalid history snapshot id");
    }
    Ok(())
}

fn format_snapshot_label(timestamp: DateTime<Utc>) -> String {
    timestamp.format("%Y-%m-%d %H:%M UTC").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn retention_bucket_uses_requested_cadence() {
        let now = Utc.with_ymd_and_hms(2026, 5, 20, 18, 17, 0).unwrap();

        assert_eq!(
            retention_bucket(now - Duration::minutes(55), now)
                .unwrap()
                .tier,
            "m5"
        );
        assert_eq!(
            retention_bucket(now - Duration::hours(47), now)
                .unwrap()
                .tier,
            "h1"
        );
        assert_eq!(
            retention_bucket(now - Duration::days(6), now).unwrap().tier,
            "h4"
        );
        assert_eq!(
            retention_bucket(now - Duration::days(300), now)
                .unwrap()
                .tier,
            "d1"
        );
        assert!(retention_bucket(now - Duration::days(366), now).is_none());
    }

    #[test]
    fn record_list_and_restore_snapshots() {
        if !git_is_available() {
            return;
        }
        let workspace_dir = unique_temp_dir("knotq-history-restore");
        fs::create_dir_all(workspace_dir.join("schemes")).unwrap();
        fs::create_dir_all(workspace_dir.join("daily_queue")).unwrap();
        fs::write(workspace_dir.join(".gitignore"), ".knotq-history/\n").unwrap();
        fs::write(workspace_dir.join("workspace.json"), "one").unwrap();
        fs::write(workspace_dir.join("schemes").join("Task.knotq"), "first").unwrap();

        let first_time = Utc
            .timestamp_opt(Utc::now().timestamp() - 10 * 60, 0)
            .unwrap();
        record_workspace_snapshot_at(&workspace_dir, first_time).unwrap();

        fs::write(workspace_dir.join("workspace.json"), "two").unwrap();
        fs::write(workspace_dir.join("schemes").join("Task.knotq"), "second").unwrap();
        record_workspace_snapshot_at(&workspace_dir, Utc::now()).unwrap();

        let snapshots = list_workspace_snapshots(&workspace_dir).unwrap();
        assert!(snapshots.len() >= 2);
        let first = snapshots
            .iter()
            .find(|snapshot| snapshot.timestamp == first_time)
            .unwrap();
        restore_workspace_snapshot(&workspace_dir, &first.id).unwrap();

        assert_eq!(
            fs::read_to_string(workspace_dir.join("workspace.json")).unwrap(),
            "one"
        );
        assert_eq!(
            fs::read_to_string(workspace_dir.join("schemes").join("Task.knotq")).unwrap(),
            "first"
        );
        assert!(workspace_dir.join(HISTORY_DIR).join("HEAD").exists());

        fs::remove_dir_all(workspace_dir).unwrap();
    }

    fn git_is_available() -> bool {
        Command::new("git")
            .arg("--version")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "{}-{}-{}",
            prefix,
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap()
        ))
    }
}
