mod cal_index;
mod files;
mod options;
mod paths;
mod schema;
mod scheme_file;
mod settings;

use async_trait::async_trait;
use chrono::NaiveDate;
use knotq_model::{Scheme, Workspace};
use knotq_storage::{LoadOptions, StorageBackend};
use std::path::{Path, PathBuf};

pub use files::{
    load_daily_queue_scheme, load_daily_queue_schemes_for_calendar_range, load_workspace,
    load_workspace_with_options, save_workspace, save_workspace_incremental,
};
pub use knotq_model::{
    AppSettings, CalendarViewMode, NotificationDefaults, SavedWindowPosition, SavedWindowSize,
    ThemeMode, TimeFormat,
};
pub use options::WorkspaceLoadOptions;
pub use paths::{data_dir, image_asset_path, image_assets_dir, settings_path, workspace_path};
pub use settings::{load_app_settings, save_app_settings};

#[derive(Clone, Debug)]
pub struct JsonBackend {
    workspace_path: PathBuf,
    settings_path: PathBuf,
}

impl JsonBackend {
    pub fn new(workspace_path: impl Into<PathBuf>, settings_path: impl Into<PathBuf>) -> Self {
        Self {
            workspace_path: workspace_path.into(),
            settings_path: settings_path.into(),
        }
    }

    pub fn from_default_paths() -> Self {
        Self::new(paths::workspace_path(), paths::settings_path())
    }

    pub fn workspace_path(&self) -> &Path {
        &self.workspace_path
    }
}

#[async_trait]
impl StorageBackend for JsonBackend {
    async fn load_workspace(&self, opts: LoadOptions) -> anyhow::Result<Workspace> {
        let file_opts = match (opts.calendar_start, opts.calendar_end) {
            (Some(start), Some(end)) => {
                options::WorkspaceLoadOptions::daily_queue_range(start, end)
            }
            _ => options::WorkspaceLoadOptions::all(),
        };
        Ok(
            files::load_workspace_with_options(&self.workspace_path, file_opts)?
                .unwrap_or_else(Workspace::new),
        )
    }

    async fn save_workspace(&self, workspace: &Workspace) -> anyhow::Result<()> {
        files::save_workspace(&self.workspace_path, workspace)
    }

    async fn load_settings(&self) -> anyhow::Result<AppSettings> {
        settings::load_app_settings(&self.settings_path)
    }

    async fn save_settings(&self, settings: &AppSettings) -> anyhow::Result<()> {
        settings::save_app_settings(&self.settings_path, settings)
    }

    async fn load_daily_queue_scheme(&self, date: NaiveDate) -> anyhow::Result<Option<Scheme>> {
        files::load_daily_queue_scheme(&self.workspace_path, date)
    }

    async fn load_daily_queue_schemes_for_calendar_range(
        &self,
        start: NaiveDate,
        end: NaiveDate,
    ) -> anyhow::Result<Vec<(NaiveDate, Scheme)>> {
        files::load_daily_queue_schemes_for_calendar_range(&self.workspace_path, start, end)
    }
}
