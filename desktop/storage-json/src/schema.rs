use anyhow::{anyhow, Result};
use chrono::NaiveDate;
use knotq_model::{DeletedSchemeOrigin, Folder, Scheme, SchemeId, SchemeSource, Workspace};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::Path,
};

use crate::{
    cal_index::SchemeCalendarIndex,
    options::WorkspaceLoadOptions,
    scheme_file::{
        read_daily_queue_file, read_scheme_file, scheme_from_index, scheme_path_for_index,
    },
};

#[derive(Serialize, Deserialize)]
pub(crate) struct WorkspaceEnvelope {
    pub(crate) version: u32,
    pub(crate) workspace: WorkspaceIndex,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct WorkspaceIndex {
    pub(crate) root: knotq_model::FolderId,
    pub(crate) folders: HashMap<knotq_model::FolderId, Folder>,
    pub(crate) schemes: HashMap<SchemeId, SchemeIndex>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) daily_queue: Vec<DailyQueueIndexEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) recently_deleted: Vec<SchemeId>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub(crate) deleted_scheme_origins: HashMap<SchemeId, DeletedSchemeOrigin>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct SchemeIndex {
    pub(crate) id: SchemeId,
    pub(crate) name: String,
    pub(crate) color_index: u8,
    #[serde(default, skip_serializing_if = "crate::files::is_false")]
    pub(crate) gsync: bool,
    #[serde(default, skip_serializing_if = "SchemeSource::is_local")]
    pub(crate) source: SchemeSource,
    #[serde(default)]
    pub(crate) calendar_index: SchemeCalendarIndex,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct DailyQueueIndexEntry {
    pub(crate) date: NaiveDate,
    pub(crate) scheme: SchemeIndex,
}

impl From<&Workspace> for WorkspaceIndex {
    fn from(workspace: &Workspace) -> Self {
        Self::from_workspace_preserving(workspace, Vec::new())
    }
}

impl WorkspaceIndex {
    pub(crate) fn from_workspace_preserving(
        workspace: &Workspace,
        existing_daily_queue: Vec<DailyQueueIndexEntry>,
    ) -> Self {
        let daily_ids: HashSet<SchemeId> = workspace.daily_queue.values().copied().collect();
        let schemes = workspace
            .schemes
            .iter()
            .filter(|(id, _)| !daily_ids.contains(id))
            .map(|(id, scheme)| (*id, SchemeIndex::from_scheme(scheme)))
            .collect();
        let existing_daily_queue: HashMap<NaiveDate, DailyQueueIndexEntry> = existing_daily_queue
            .into_iter()
            .map(|entry| (entry.date, entry))
            .collect();
        let daily_queue = workspace
            .daily_queue
            .iter()
            .filter_map(|(date, id)| match workspace.schemes.get(id) {
                Some(scheme) => Some(DailyQueueIndexEntry {
                    date: *date,
                    scheme: SchemeIndex::from_scheme(scheme),
                }),
                None => existing_daily_queue
                    .get(date)
                    .filter(|entry| entry.scheme.id == *id)
                    .cloned(),
            })
            .collect();

        Self {
            root: workspace.root,
            folders: workspace.folders.clone(),
            schemes,
            daily_queue,
            recently_deleted: workspace.recently_deleted.clone(),
            deleted_scheme_origins: workspace.deleted_scheme_origins.clone(),
        }
    }

    pub(crate) fn into_workspace_with_options(
        self,
        base_dir: &Path,
        options: WorkspaceLoadOptions,
    ) -> Result<Workspace> {
        let WorkspaceIndex {
            root,
            folders,
            schemes: scheme_index,
            daily_queue: daily_queue_index,
            recently_deleted,
            deleted_scheme_origins,
        } = self;
        let mut schemes = HashMap::with_capacity(scheme_index.len() + daily_queue_index.len());
        for (id, index) in scheme_index {
            let path = scheme_path_for_index(
                base_dir,
                root,
                &folders,
                &recently_deleted,
                id,
                &index.name,
            )?;
            let file = read_scheme_file(&path, id)?;
            if file.id != id {
                return Err(anyhow!(
                    "scheme file {} contains id {}",
                    path.display(),
                    file.id
                ));
            }
            schemes.insert(id, scheme_from_index(index, file.items));
        }
        let mut daily_queue = BTreeMap::new();
        for entry in daily_queue_index {
            let id = entry.scheme.id;
            daily_queue.insert(entry.date, id);
            if !options.should_load_daily_queue_entry(&entry) {
                continue;
            }
            let Ok(file) = read_daily_queue_file(base_dir, entry.date, id) else {
                continue;
            };
            if file.id != id {
                return Err(anyhow!(
                    "daily queue file {} contains id {}",
                    crate::paths::daily_queue_file_path(base_dir, entry.date).display(),
                    file.id
                ));
            }
            schemes.insert(id, scheme_from_index(entry.scheme, file.items));
        }

        Ok(Workspace {
            root,
            folders,
            schemes,
            daily_queue,
            recently_deleted,
            deleted_scheme_origins,
        })
    }
}

impl SchemeIndex {
    pub(crate) fn from_scheme(scheme: &Scheme) -> Self {
        Self {
            id: scheme.id,
            name: scheme.name.clone(),
            color_index: scheme.color_index,
            gsync: scheme.gsync,
            source: scheme.source.clone(),
            calendar_index: SchemeCalendarIndex::from_scheme(scheme),
        }
    }
}
