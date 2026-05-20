use async_trait::async_trait;
use chrono::NaiveDate;
use knotq_model::{AppSettings, Scheme, Workspace};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LoadOptions {
    pub include_daily_queue_schemes: bool,
    pub calendar_start: Option<NaiveDate>,
    pub calendar_end: Option<NaiveDate>,
}

pub type WorkspaceLoadOptions = LoadOptions;

#[async_trait]
pub trait StorageBackend: Send + Sync {
    async fn load_workspace(&self, opts: LoadOptions) -> anyhow::Result<Workspace>;
    async fn save_workspace(&self, workspace: &Workspace) -> anyhow::Result<()>;
    async fn load_settings(&self) -> anyhow::Result<AppSettings>;
    async fn save_settings(&self, settings: &AppSettings) -> anyhow::Result<()>;
    async fn load_daily_queue_scheme(&self, date: NaiveDate) -> anyhow::Result<Option<Scheme>>;
    async fn load_daily_queue_schemes_for_calendar_range(
        &self,
        start: NaiveDate,
        end: NaiveDate,
    ) -> anyhow::Result<Vec<(NaiveDate, Scheme)>>;
}
