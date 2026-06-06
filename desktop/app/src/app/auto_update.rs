use std::path::PathBuf;
use std::time::Duration as StdDuration;

use chrono::{DateTime, Utc};
use futures::{pin_mut, select, FutureExt};
use gpui::{Context, Task};
use knotq_auto_update::{
    check_latest_release, current_version, open_staged_installer, prepare_update,
    run_windows_installer, AutoUpdateConfig, AvailableUpdate, InstallStrategy, LatestRelease,
    StagedUpdate,
};
use knotq_storage_json::data_dir;

use super::KnotQApp;

const AUTO_UPDATE_STARTUP_DELAY: StdDuration = StdDuration::ZERO;
const AUTO_UPDATE_POLL_INTERVAL: StdDuration = StdDuration::from_secs(30 * 60);

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
        matches!(self, Self::Checking | Self::Downloading { .. })
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

    fn has_actionable_update(&self) -> bool {
        matches!(self, Self::Available { .. } | Self::Ready { .. })
            || matches!(
                self,
                Self::Errored {
                    update: Some(_),
                    ..
                }
            )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum AutoUpdateSignal {
    CheckNow,
    CheckNowAutomatic,
    Download(AvailableUpdate),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AutoUpdateCheckKind {
    Automatic,
    Manual,
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
                    AutoUpdateSignal::Download(update) => {
                        prepare_available_update(&weak, cx, update, AutoUpdateCheckKind::Manual)
                            .await;
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
        };
        let _ = self
            .auto_update_tx
            .try_send(AutoUpdateSignal::Download(update));
        cx.notify();
    }

    pub fn install_ready_update(&mut self, cx: &mut Context<Self>) {
        let AutoUpdateUiStatus::Ready { update } = self.auto_update_status.clone() else {
            return;
        };

        match update.install_strategy {
            InstallStrategy::InstalledOnRestart => {
                self.flush_for_shutdown("auto update restart");
                cx.restart();
            }
            InstallStrategy::OpenInstaller => match open_staged_installer(&update) {
                Ok(()) => {}
                Err(err) => {
                    self.auto_update_status = AutoUpdateUiStatus::Errored {
                        message: format!("Could not open update installer: {err:#}"),
                        checked_at: Utc::now(),
                        update: None,
                    };
                    cx.notify();
                }
            },
            InstallStrategy::RunInstallerAndQuit => match run_windows_installer(&update) {
                Ok(()) => {
                    self.flush_for_shutdown("auto update installer");
                    cx.quit();
                }
                Err(err) => {
                    self.auto_update_status = AutoUpdateUiStatus::Errored {
                        message: format!("Could not launch installer: {err:#}"),
                        checked_at: Utc::now(),
                        update: None,
                    };
                    cx.notify();
                }
            },
        }
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
    let update_actionable = weak
        .update(cx, |app, _cx| {
            app.auto_update_status.has_actionable_update()
        })
        .unwrap_or(false);
    if kind == AutoUpdateCheckKind::Automatic && update_actionable {
        return;
    }

    set_update_status(weak, cx, AutoUpdateUiStatus::Checking);

    let current_version = match current_version(env!("CARGO_PKG_VERSION")) {
        Ok(version) => version,
        Err(err) => {
            set_check_error(weak, cx, kind, format!("Invalid app version: {err:#}"));
            return;
        }
    };
    let config = AutoUpdateConfig::github(current_version);
    let latest = cx
        .background_executor()
        .spawn({
            let config = config.clone();
            async move { check_latest_release(&config) }
        })
        .await;

    match latest {
        Ok(LatestRelease::UpToDate { version, .. }) => {
            let status = if kind == AutoUpdateCheckKind::Manual {
                AutoUpdateUiStatus::UpToDate {
                    version: version.to_string(),
                    checked_at: Utc::now(),
                }
            } else {
                AutoUpdateUiStatus::Idle
            };
            set_update_status(weak, cx, status);
            return;
        }
        Ok(LatestRelease::Available(update)) => {
            set_update_status(
                weak,
                cx,
                AutoUpdateUiStatus::Available {
                    update,
                    checked_at: Utc::now(),
                },
            );
        }
        Err(err) => {
            set_check_error(
                weak,
                cx,
                kind,
                format!("Could not check for updates: {err:#}"),
            );
        }
    }
}

async fn prepare_available_update(
    weak: &gpui::WeakEntity<KnotQApp>,
    cx: &mut gpui::AsyncApp,
    update: AvailableUpdate,
    kind: AutoUpdateCheckKind,
) {
    set_update_status(
        weak,
        cx,
        AutoUpdateUiStatus::Downloading {
            version: update.version.to_string(),
        },
    );

    let app_path = match running_app_path(cx) {
        Ok(path) => path,
        Err(err) => {
            set_check_error(
                weak,
                cx,
                kind,
                format!("Could not locate the running app: {err:#}"),
            );
            return;
        }
    };
    let current_version = match current_version(env!("CARGO_PKG_VERSION")) {
        Ok(version) => version,
        Err(err) => {
            set_check_error(weak, cx, kind, format!("Invalid app version: {err:#}"));
            return;
        }
    };
    let config = AutoUpdateConfig::github(current_version);
    let updates_dir = data_dir().join("updates");
    let prepared = cx
        .background_executor()
        .spawn({
            let config = config.clone();
            let update = update.clone();
            async move { prepare_update(&config, &update, &app_path, &updates_dir) }
        })
        .await;

    match prepared {
        Ok(staged) => {
            if let Some(path) = staged.restart_path.clone() {
                let _ = cx.update(|cx| cx.set_restart_path(path));
            }
            set_update_status(weak, cx, AutoUpdateUiStatus::Ready { update: staged });
        }
        Err(err) => {
            set_prepare_error(
                weak,
                cx,
                kind,
                update,
                format!("Could not prepare update: {err:#}"),
            );
        }
    }
}

fn running_app_path(cx: &mut gpui::AsyncApp) -> anyhow::Result<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        cx.update(|cx| cx.app_path())?
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
        eprintln!("auto-update prepare failed: {message}");
        set_update_status(weak, cx, AutoUpdateUiStatus::Idle);
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
