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

// `async_trait` rewrites each method to return a boxed future and marks it
// `#[must_use]`; every method here returns `anyhow::Result`, which is `#[must_use]`
// too, so clippy 1.99 reports `double_must_use` once per method — six errors from
// the macro expansion, none of them from anything written here. The attribute is on
// the expansion, so it cannot be removed; allowing it at the trait is the only
// place that covers all six. CI has been red on this since stable moved to 1.99
// (main's own run on 2026-10-01).
#[allow(clippy::double_must_use)]
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
