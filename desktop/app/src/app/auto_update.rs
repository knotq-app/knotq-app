use std::path::PathBuf;
use std::time::Duration as StdDuration;

use chrono::{DateTime, Utc};
use futures::{pin_mut, select, FutureExt};
use gpui::{Context, Task};
use knotq_auto_update::{
    check_latest_release, check_update_newer_than, current_version, discard_installed_downloads,
    install_staged_update, prepare_update, refresh_available_update, AutoUpdateConfig,
    AvailableUpdate, InstallStrategy, LatestRelease, StagedUpdate,
};
use knotq_storage_json::data_dir;

use super::sync_auth::analytics_api_base;
use super::KnotQApp;

/// Long enough for the launch to finish — loading the workspace, the first
/// paint, the first sync — before an update download starts competing with it.
const AUTO_UPDATE_STARTUP_DELAY: StdDuration = StdDuration::from_secs(20);
const AUTO_UPDATE_POLL_INTERVAL: StdDuration = StdDuration::from_secs(30 * 60);
/// How old a staged update may be before taking it re-checks for a newer release
/// first. Inside this window the restart is immediate: no network round trip
/// between the click and the install.
const STAGED_UPDATE_FRESH_FOR: chrono::Duration = chrono::Duration::hours(6);
const AUTO_UPDATE_LOG_FILE: &str = "knotq-update.log";

/// Record what the updater did and how long it took.
///
/// "Updating takes forever" is the one report about this feature that cannot be
/// acted on without knowing which step the time went into — the download, the
/// unpacking, or the install — so each one says so here.
fn update_log(message: impl AsRef<str>) {
    knotq_storage_json::append_diagnostic_line(
        &data_dir(),
        AUTO_UPDATE_LOG_FILE,
        &format!("[{}] {}", Utc::now().format("%H:%M:%S"), message.as_ref()),
    );
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AutoUpdateUiStatus {
    Idle,
    Checking,
    Available {
        update: AvailableUpdate,
        checked_at: DateTime<Utc>,
    },
    Downloading {
        version: String,
        /// Whether the user asked for this download. A download staged in the
        /// background stays out of the title bar: the button appears when the
        /// update is ready to install, not while it is being fetched.
        user_initiated: bool,
    },
    Installing {
        version: String,
    },
    Ready {
        update: StagedUpdate,
    },
    UpToDate {
        version: String,
        checked_at: DateTime<Utc>,
    },
    Errored {
        message: String,
        checked_at: DateTime<Utc>,
        update: Option<AvailableUpdate>,
    },
}

impl AutoUpdateUiStatus {
    pub fn initial() -> Self {
        Self::Idle
    }

    pub fn is_busy(&self) -> bool {
        matches!(
            self,
            Self::Checking | Self::Downloading { .. } | Self::Installing { .. }
        )
    }

    pub fn available_update(&self) -> Option<AvailableUpdate> {
        match self {
            Self::Available { update, .. } => Some(update.clone()),
            Self::Errored {
                update: Some(update),
                ..
            } => Some(update.clone()),
            _ => None,
        }
    }

    /// Whether a periodic check would undo something better than itself.
    ///
    /// A staged update is left alone — it is the end state, and re-checking would
    /// only risk replacing it. Anything else, including an update that is known
    /// about but failed to stage, is worth another attempt: the check that
    /// follows will try to stage it again.
    fn blocks_automatic_check(&self) -> bool {
        self.is_busy() || matches!(self, Self::Ready { .. })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum AutoUpdateSignal {
    CheckNow,
    CheckNowAutomatic,
    Download { update: AvailableUpdate },
    Install { update: StagedUpdate },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AutoUpdateCheckKind {
    Automatic,
    Manual,
}

/// Why an update is being downloaded, which decides both how visible it is and
/// what happens once it is staged.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PrepareReason {
    /// Staged ahead of being asked for, so that the restart is quick when the
    /// user does ask. Silent, and stops at `Ready`.
    Background,
    /// The user asked for the update now: show the progress, and install it as
    /// soon as it is staged.
    Requested,
}

impl PrepareReason {
    fn install_when_ready(self) -> bool {
        self == Self::Requested
    }

    fn user_initiated(self) -> bool {
        self == Self::Requested
    }

    /// Whether to re-check the release manifest before downloading. Pointless
    /// when the caller has just fetched it.
    fn refresh_first(self) -> bool {
        self == Self::Requested
    }
}

pub(crate) fn spawn_auto_update_task(
    rx: async_channel::Receiver<AutoUpdateSignal>,
    cx: &mut Context<KnotQApp>,
) -> Task<()> {
    cx.spawn(
        async move |weak: gpui::WeakEntity<KnotQApp>, cx: &mut gpui::AsyncApp| {
            let mut delay = AUTO_UPDATE_STARTUP_DELAY;
            loop {
                let timer = cx.background_executor().timer(delay).fuse();
                let signal = rx.recv().fuse();
                pin_mut!(timer, signal);

                let signal = select! {
                    _ = timer => AutoUpdateSignal::CheckNowAutomatic,
                    signal = signal => {
                        match signal {
                            Ok(signal) => signal,
                            Err(_) => break,
                        }
                    }
                };

                match signal {
                    AutoUpdateSignal::CheckNow => {
                        run_update_check(&weak, cx, AutoUpdateCheckKind::Manual).await;
                    }
                    AutoUpdateSignal::CheckNowAutomatic => {
                        run_update_check(&weak, cx, AutoUpdateCheckKind::Automatic).await;
                    }
                    AutoUpdateSignal::Download { update } => {
                        prepare_available_update(
                            &weak,
                            cx,
                            update,
                            AutoUpdateCheckKind::Manual,
                            PrepareReason::Requested,
                        )
                        .await;
                    }
                    AutoUpdateSignal::Install { update } => {
                        install_or_refresh_ready_update(&weak, cx, update).await;
                    }
                }
                delay = AUTO_UPDATE_POLL_INTERVAL;
            }
        },
    )
}

impl KnotQApp {
    pub fn set_auto_update_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.settings.auto_update == enabled {
            return;
        }
        self.settings.auto_update = enabled;
        if enabled {
            self.auto_update_status = AutoUpdateUiStatus::initial();
            let _ = self.auto_update_tx.try_send(AutoUpdateSignal::CheckNow);
        } else {
            self.auto_update_status = AutoUpdateUiStatus::Idle;
        }
        self.save_app_settings();
        cx.notify();
    }

    pub fn check_for_updates(&mut self, cx: &mut Context<Self>) {
        if self.auto_update_status.is_busy() {
            return;
        }
        self.auto_update_status = AutoUpdateUiStatus::Checking;
        let _ = self.auto_update_tx.try_send(AutoUpdateSignal::CheckNow);
        cx.notify();
    }

    pub fn download_available_update(&mut self, cx: &mut Context<Self>) {
        if self.auto_update_status.is_busy() {
            return;
        }
        let Some(update) = self.auto_update_status.available_update() else {
            return;
        };
        self.auto_update_status = AutoUpdateUiStatus::Downloading {
            version: update.version.to_string(),
            user_initiated: true,
        };
        let _ = self
            .auto_update_tx
            .try_send(AutoUpdateSignal::Download { update });
        cx.notify();
    }

    pub fn install_ready_update(&mut self, cx: &mut Context<Self>) {
        let AutoUpdateUiStatus::Ready { update } = self.auto_update_status.clone() else {
            return;
        };

        self.auto_update_status = AutoUpdateUiStatus::Installing {
            version: update.version.to_string(),
        };
        let _ = self
            .auto_update_tx
            .try_send(AutoUpdateSignal::Install { update });
        cx.notify();
    }

    /// Copy the staged payload into place and restart.
    ///
    /// The install runs on the background executor: it rewrites every file of
    /// the app bundle, and doing that on the foreground froze the window for the
    /// whole copy — which is what made taking an update look like a hang.
    ///
    /// The shutdown flush stays *after* the install, so that an edit made while
    /// the copy is running is still saved. That ordering is safe because every
    /// file is replaced by rename: this process keeps the inodes it already
    /// mapped, and only the relaunch sees the new ones.
    fn install_staged_update_now(&mut self, update: StagedUpdate, cx: &mut Context<Self>) {
        let install_strategy = update.install_strategy.clone();
        self.auto_update_status = AutoUpdateUiStatus::Installing {
            version: update.version.to_string(),
        };
        cx.notify();

        let version = update.version.to_string();
        update_log(format!("installing {version}"));
        cx.spawn(async move |weak, cx| {
            let started = std::time::Instant::now();
            let result = cx
                .background_executor()
                .spawn(async move { install_staged_update(&update) })
                .await;
            let elapsed = started.elapsed().as_secs_f64();
            match &result {
                Ok(_) => update_log(format!("{version} installed in {elapsed:.1}s, restarting")),
                Err(err) => update_log(format!(
                    "installing {version} failed after {elapsed:.1}s: {err:#}"
                )),
            }
            let _ = weak.update(cx, |app, cx| match result {
                Ok(restart_path) => {
                    if let Some(path) = restart_path {
                        cx.set_restart_path(path);
                    }
                    match install_strategy {
                        InstallStrategy::InstalledOnRestart => {
                            app.flush_for_shutdown("auto update restart");
                            cx.restart();
                        }
                        InstallStrategy::RunInstallerAndQuit => {
                            app.flush_for_shutdown("auto update installer");
                            cx.quit();
                        }
                    }
                }
                Err(err) => {
                    let key = match install_strategy {
                        InstallStrategy::InstalledOnRestart => "update.error.install_failed",
                        InstallStrategy::RunInstallerAndQuit => {
                            "update.error.launch_installer_failed"
                        }
                    };
                    app.auto_update_status = AutoUpdateUiStatus::Errored {
                        message: knotq_l10n::t_with(key, &[("error", &format!("{err:#}"))]),
                        checked_at: Utc::now(),
                        update: None,
                    };
                    cx.notify();
                }
            });
        })
        .detach();
    }
}

async fn run_update_check(
    weak: &gpui::WeakEntity<KnotQApp>,
    cx: &mut gpui::AsyncApp,
    kind: AutoUpdateCheckKind,
) {
    let enabled = weak
        .update(cx, |app, _cx| app.settings.auto_update)
        .unwrap_or(false);
    if kind == AutoUpdateCheckKind::Automatic && !enabled {
        return;
    }
    let already_settled = weak
        .update(cx, |app, _cx| {
            app.auto_update_status.blocks_automatic_check()
        })
        .unwrap_or(false);
    if kind == AutoUpdateCheckKind::Automatic && already_settled {
        return;
    }

    set_update_status(weak, cx, AutoUpdateUiStatus::Checking);

    let current_version = match current_version(env!("CARGO_PKG_VERSION")) {
        Ok(version) => version,
        Err(err) => {
            set_check_error(
                weak,
                cx,
                kind,
                knotq_l10n::t_with(
                    "update.error.invalid_version",
                    &[("error", &format!("{err:#}"))],
                ),
            );
            return;
        }
    };
    let config = AutoUpdateConfig::github(current_version.clone());
    let latest = cx
        .background_executor()
        .spawn({
            let config = config.clone();
            async move { check_latest_release(&config) }
        })
        .await;

    match latest {
        Ok(LatestRelease::UpToDate { version, .. }) => {
            update_log(format!("up to date at {version}"));
            // The first check after an update is where the payload it was
            // installed from gets cleaned up — the app is up to date, so nothing
            // else will ever come back for it.
            cx.background_executor()
                .spawn(async move {
                    discard_installed_downloads(&data_dir().join("updates"), &current_version);
                })
                .await;
            let status = if kind == AutoUpdateCheckKind::Manual {
                AutoUpdateUiStatus::UpToDate {
                    version: version.to_string(),
                    checked_at: Utc::now(),
                }
            } else {
                AutoUpdateUiStatus::Idle
            };
            set_update_status(weak, cx, status);
        }
        Ok(LatestRelease::Available(update)) => {
            set_update_status(
                weak,
                cx,
                AutoUpdateUiStatus::Available {
                    update: update.clone(),
                    checked_at: Utc::now(),
                },
            );
            update_log(format!(
                "{} available (current {})",
                update.version,
                env!("CARGO_PKG_VERSION")
            ));
            // Download and unpack it now, while nobody is waiting. Taking the
            // update then costs a file copy and a relaunch instead of a download.
            if enabled {
                prepare_available_update(weak, cx, update, kind, PrepareReason::Background).await;
            }
        }
        Err(err) => {
            set_check_error(
                weak,
                cx,
                kind,
                knotq_l10n::t_with(
                    "update.error.check_failed",
                    &[("error", &format!("{err:#}"))],
                ),
            );
        }
    }
}

/// Download and unpack an update, and — if the user is waiting for it — install
/// it as soon as that is done.
///
/// Every expensive step runs on the background executor. The foreground only
/// swaps the status label, so the window keeps painting throughout.
async fn prepare_available_update(
    weak: &gpui::WeakEntity<KnotQApp>,
    cx: &mut gpui::AsyncApp,
    update: AvailableUpdate,
    kind: AutoUpdateCheckKind,
    reason: PrepareReason,
) {
    set_update_status(
        weak,
        cx,
        AutoUpdateUiStatus::Downloading {
            version: update.version.to_string(),
            user_initiated: reason.user_initiated(),
        },
    );

    let app_path = match running_app_path(cx) {
        Ok(path) => path,
        Err(err) => {
            set_check_error(
                weak,
                cx,
                kind,
                knotq_l10n::t_with(
                    "update.error.locate_app_failed",
                    &[("error", &format!("{err:#}"))],
                ),
            );
            return;
        }
    };
    let current_version = match current_version(env!("CARGO_PKG_VERSION")) {
        Ok(version) => version,
        Err(err) => {
            set_check_error(
                weak,
                cx,
                kind,
                knotq_l10n::t_with(
                    "update.error.invalid_version",
                    &[("error", &format!("{err:#}"))],
                ),
            );
            return;
        }
    };
    let config = AutoUpdateConfig::github(current_version);
    // A button the user clicked may have been on screen for a while, so confirm
    // it is still the newest release. A background stage follows a check that
    // just happened, and re-fetching the manifest would tell it nothing new.
    let update = if reason.refresh_first() {
        let selected_update = update.clone();
        let refreshed = cx
            .background_executor()
            .spawn({
                let config = config.clone();
                async move { refresh_available_update(&config, &selected_update) }
            })
            .await;
        match refreshed {
            Ok(refreshed) => {
                set_update_status(
                    weak,
                    cx,
                    AutoUpdateUiStatus::Downloading {
                        version: refreshed.version.to_string(),
                        user_initiated: reason.user_initiated(),
                    },
                );
                refreshed
            }
            Err(err) => {
                set_prepare_error(
                    weak,
                    cx,
                    kind,
                    update,
                    knotq_l10n::t_with(
                        "update.error.check_failed",
                        &[("error", &format!("{err:#}"))],
                    ),
                );
                return;
            }
        }
    } else {
        update
    };
    let updates_dir = data_dir().join("updates");
    let analytics_base = analytics_api_base();
    update_log(format!(
        "staging {} ({})",
        update.version,
        match reason {
            PrepareReason::Background => "background",
            PrepareReason::Requested => "requested",
        }
    ));
    let staging_started = std::time::Instant::now();
    let prepared = cx
        .background_executor()
        .spawn({
            let config = config.clone();
            let update = update.clone();
            async move {
                record_update_download(&analytics_base, "update_download_started", &update.version);
                let result = prepare_update(&config, &update, &app_path, &updates_dir);
                if result.is_ok() {
                    record_update_download(
                        &analytics_base,
                        "update_download_verified",
                        &update.version,
                    );
                }
                result
            }
        })
        .await;

    match prepared {
        Ok(staged) => {
            update_log(format!(
                "{} staged in {:.1}s, ready to install",
                staged.version,
                staging_started.elapsed().as_secs_f64()
            ));
            if let Some(path) = staged.restart_path.clone() {
                let _ = cx.update(|cx| cx.set_restart_path(path));
            }
            if reason.install_when_ready() {
                let _ = weak.update(cx, |app, cx| {
                    app.auto_update_status = AutoUpdateUiStatus::Ready { update: staged };
                    app.install_ready_update(cx);
                });
            } else {
                set_update_status(weak, cx, AutoUpdateUiStatus::Ready { update: staged });
            }
        }
        Err(err) => {
            update_log(format!(
                "staging {} failed after {:.1}s: {err:#}",
                update.version,
                staging_started.elapsed().as_secs_f64()
            ));
            set_prepare_error(
                weak,
                cx,
                kind,
                update,
                knotq_l10n::t_with(
                    "update.error.prepare_failed",
                    &[("error", &format!("{err:#}"))],
                ),
            );
        }
    }
}

/// Best-effort, aggregate-only telemetry. A failed request must never affect an
/// update. The payload contains only the platform, event, and release version.
fn record_update_download(api_base: &str, event: &str, version: &impl std::fmt::Display) {
    let url = format!("{}/v1/analytics/events", api_base.trim_end_matches('/'));
    let _ = ureq::post(&url)
        .set("content-type", "application/json")
        .set("user-agent", concat!("KnotQ/", env!("CARGO_PKG_VERSION")))
        .send_json(ureq::json!({
            "event": event,
            "platform": std::env::consts::OS,
            "device": "desktop",
            "release_version": version.to_string(),
        }));
}

/// Take a staged update.
///
/// Nothing on this path may wait on the network: it runs when the user has just
/// clicked restart. A staged update that is still fresh is installed straight
/// away; only one left over from hours ago is checked against the latest release
/// first, so that a long-running window does not install a superseded build.
async fn install_or_refresh_ready_update(
    weak: &gpui::WeakEntity<KnotQApp>,
    cx: &mut gpui::AsyncApp,
    staged: StagedUpdate,
) {
    if Utc::now() - staged.staged_at < STAGED_UPDATE_FRESH_FOR && staged.payload_is_present() {
        let _ = weak.update(cx, |app, cx| {
            app.install_staged_update_now(staged, cx);
        });
        return;
    }

    let current_version = match current_version(env!("CARGO_PKG_VERSION")) {
        Ok(version) => version,
        Err(err) => {
            set_update_status(
                weak,
                cx,
                AutoUpdateUiStatus::Errored {
                    message: knotq_l10n::t_with(
                        "update.error.invalid_version",
                        &[("error", &format!("{err:#}"))],
                    ),
                    checked_at: Utc::now(),
                    update: None,
                },
            );
            return;
        }
    };
    let config = AutoUpdateConfig::github(current_version);
    let newer = cx
        .background_executor()
        .spawn({
            let config = config.clone();
            let version = staged.version.clone();
            async move { check_update_newer_than(&config, &version) }
        })
        .await;

    match newer {
        Ok(Some(update)) => {
            prepare_available_update(
                weak,
                cx,
                update,
                AutoUpdateCheckKind::Manual,
                PrepareReason::Requested,
            )
            .await;
        }
        Ok(None) | Err(_) => {
            let _ = weak.update(cx, |app, cx| {
                app.auto_update_status = AutoUpdateUiStatus::Ready {
                    update: staged.clone(),
                };
                app.install_staged_update_now(staged, cx);
            });
        }
    }
}

fn running_app_path(_cx: &mut gpui::AsyncApp) -> anyhow::Result<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        _cx.update(|cx| cx.app_path())?
    }

    #[cfg(not(target_os = "macos"))]
    {
        std::env::current_exe().map_err(Into::into)
    }
}

fn set_check_error(
    weak: &gpui::WeakEntity<KnotQApp>,
    cx: &mut gpui::AsyncApp,
    kind: AutoUpdateCheckKind,
    message: String,
) {
    if kind == AutoUpdateCheckKind::Automatic {
        eprintln!("auto-update check failed: {message}");
        set_update_status(weak, cx, AutoUpdateUiStatus::Idle);
    } else {
        set_update_status(
            weak,
            cx,
            AutoUpdateUiStatus::Errored {
                message,
                checked_at: Utc::now(),
                update: None,
            },
        );
    }
}

fn set_prepare_error(
    weak: &gpui::WeakEntity<KnotQApp>,
    cx: &mut gpui::AsyncApp,
    kind: AutoUpdateCheckKind,
    update: AvailableUpdate,
    message: String,
) {
    if kind == AutoUpdateCheckKind::Automatic {
        // Staging in the background is allowed to fail quietly, but the update
        // itself must not disappear with it: the button stays, so the user can
        // still take the update by hand, and the next check stages it again.
        eprintln!("auto-update prepare failed: {message}");
        set_update_status(
            weak,
            cx,
            AutoUpdateUiStatus::Available {
                update,
                checked_at: Utc::now(),
            },
        );
    } else {
        set_update_status(
            weak,
            cx,
            AutoUpdateUiStatus::Errored {
                message,
                checked_at: Utc::now(),
                update: Some(update),
            },
        );
    }
}

fn set_update_status(
    weak: &gpui::WeakEntity<KnotQApp>,
    cx: &mut gpui::AsyncApp,
    status: AutoUpdateUiStatus,
) {
    let _ = weak.update(cx, |app, cx| {
        app.auto_update_status = status;
        cx.notify();
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::title_bar::{title_bar_update_state, TitleUpdateState};
    use knotq_auto_update::ReleaseAsset;
    use std::path::PathBuf;

    fn available() -> AvailableUpdate {
        AvailableUpdate {
            version: current_version("0.60.0").unwrap(),
            tag_name: "v0.60.0".into(),
            release_url: "https://example.test/notes".into(),
            asset: ReleaseAsset {
                os: "macos".into(),
                arch: "aarch64".into(),
                kind: "dmg".into(),
                name: "KnotQ-0.60.0-macos-arm64.dmg".into(),
                download_url: "https://example.test/KnotQ.dmg".into(),
                sha256: "a".repeat(64),
            },
        }
    }

    fn staged(staged_at: DateTime<Utc>) -> StagedUpdate {
        StagedUpdate {
            version: current_version("0.60.0").unwrap(),
            tag_name: "v0.60.0".into(),
            release_url: "https://example.test/notes".into(),
            asset_name: "KnotQ-0.60.0-macos-arm64.dmg".into(),
            asset_path: PathBuf::from("/tmp/knotq/updates/0.60.0/KnotQ.dmg"),
            payload_path: Some(PathBuf::from("/tmp/knotq/updates/0.60.0/payload/KnotQ.app")),
            restart_path: None,
            install_strategy: InstallStrategy::InstalledOnRestart,
            staged_at,
        }
    }

    /// The behaviour the whole change is for: while an update is being fetched
    /// without being asked for, the title bar says nothing. The button appears
    /// only once the update is staged, and then it restarts rather than downloads.
    #[test]
    fn the_title_bar_offers_nothing_until_an_update_is_staged() {
        assert_eq!(
            title_bar_update_state(&AutoUpdateUiStatus::Downloading {
                version: "0.60.0".into(),
                user_initiated: false,
            }),
            TitleUpdateState::Hidden
        );
        assert_eq!(
            title_bar_update_state(&AutoUpdateUiStatus::Ready {
                update: staged(Utc::now())
            }),
            TitleUpdateState::Install {
                version: "0.60.0".into(),
                strategy: InstallStrategy::InstalledOnRestart,
            }
        );
    }

    /// A download the user clicked for still reports progress — they are waiting
    /// on it — and so does the install that follows.
    #[test]
    fn a_requested_update_reports_its_progress() {
        assert_eq!(
            title_bar_update_state(&AutoUpdateUiStatus::Downloading {
                version: "0.60.0".into(),
                user_initiated: true,
            }),
            TitleUpdateState::Working {
                version: "0.60.0".into()
            }
        );
        assert_eq!(
            title_bar_update_state(&AutoUpdateUiStatus::Installing {
                version: "0.60.0".into()
            }),
            TitleUpdateState::Working {
                version: "0.60.0".into()
            }
        );
    }

    /// Nothing to act on means nothing in the title bar.
    #[test]
    fn states_without_an_update_show_no_button() {
        for status in [
            AutoUpdateUiStatus::Idle,
            AutoUpdateUiStatus::Checking,
            AutoUpdateUiStatus::UpToDate {
                version: "0.60.0".into(),
                checked_at: Utc::now(),
            },
            AutoUpdateUiStatus::Errored {
                message: "no network".into(),
                checked_at: Utc::now(),
                update: None,
            },
        ] {
            assert_eq!(
                title_bar_update_state(&status),
                TitleUpdateState::Hidden,
                "{status:?} should not put a button in the title bar"
            );
        }
    }

    /// An update that is known about but not staged — auto-update turned off, or
    /// a background download that failed — still offers the manual path.
    #[test]
    fn an_unstaged_update_can_still_be_taken_by_hand() {
        assert_eq!(
            title_bar_update_state(&AutoUpdateUiStatus::Available {
                update: available(),
                checked_at: Utc::now(),
            }),
            TitleUpdateState::Download {
                version: "0.60.0".into(),
                retry: false,
            }
        );
        assert_eq!(
            title_bar_update_state(&AutoUpdateUiStatus::Errored {
                message: "download failed".into(),
                checked_at: Utc::now(),
                update: Some(available()),
            }),
            TitleUpdateState::Download {
                version: "0.60.0".into(),
                retry: true,
            }
        );
    }

    /// A periodic check must not disturb a staged update, and must not run while
    /// one is already being fetched or installed — but an update that failed to
    /// stage has to be retried, or a single flaky download would strand the user
    /// on an old build until they restarted.
    #[test]
    fn periodic_checks_retry_a_failed_stage_but_leave_a_staged_update_alone() {
        assert!(AutoUpdateUiStatus::Ready {
            update: staged(Utc::now())
        }
        .blocks_automatic_check());
        assert!(AutoUpdateUiStatus::Checking.blocks_automatic_check());
        assert!(AutoUpdateUiStatus::Downloading {
            version: "0.60.0".into(),
            user_initiated: false,
        }
        .blocks_automatic_check());
        assert!(AutoUpdateUiStatus::Installing {
            version: "0.60.0".into()
        }
        .blocks_automatic_check());

        assert!(!AutoUpdateUiStatus::Available {
            update: available(),
            checked_at: Utc::now(),
        }
        .blocks_automatic_check());
        assert!(!AutoUpdateUiStatus::Errored {
            message: "download failed".into(),
            checked_at: Utc::now(),
            update: Some(available()),
        }
        .blocks_automatic_check());
        assert!(!AutoUpdateUiStatus::Idle.blocks_automatic_check());
    }

    /// Clicking restart must not wait on the network. A freshly staged update is
    /// installed straight away; one left over from a long-running window is
    /// checked against the latest release first.
    #[test]
    fn only_a_stale_staged_update_is_rechecked_before_installing() {
        let fresh = staged(Utc::now() - chrono::Duration::minutes(31));
        assert!(Utc::now() - fresh.staged_at < STAGED_UPDATE_FRESH_FOR);

        let stale = staged(Utc::now() - chrono::Duration::hours(7));
        assert!(Utc::now() - stale.staged_at >= STAGED_UPDATE_FRESH_FOR);
    }

    /// A background stage is quiet and stops when it is ready; a requested one
    /// shows progress and carries straight on into the install.
    #[test]
    fn the_reason_for_a_download_decides_whether_it_installs_itself() {
        assert!(!PrepareReason::Background.install_when_ready());
        assert!(!PrepareReason::Background.user_initiated());
        assert!(!PrepareReason::Background.refresh_first());

        assert!(PrepareReason::Requested.install_when_ready());
        assert!(PrepareReason::Requested.user_initiated());
        assert!(PrepareReason::Requested.refresh_first());
    }

    /// Retrying a failed download needs the update that failed, whichever status
    /// it ended up in.
    #[test]
    fn a_failed_download_keeps_the_update_it_failed_on() {
        assert!(AutoUpdateUiStatus::Errored {
            message: "download failed".into(),
            checked_at: Utc::now(),
            update: Some(available()),
        }
        .available_update()
        .is_some());
        assert!(AutoUpdateUiStatus::Errored {
            message: "check failed".into(),
            checked_at: Utc::now(),
            update: None,
        }
        .available_update()
        .is_none());
    }
}
