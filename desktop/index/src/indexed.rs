use knotq_model::{FolderId, SchemeId, Workspace};
use knotq_rrule::{DefaultExpander, OccurrenceExpander};

use crate::calendar::{build_calendar_index, update_calendar_index, CalendarIndex};
use crate::channel::{build_channel_index, update_channel_index, ChannelIndex};
use crate::search::{build_search_index, update_search_index, SearchIndex};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IndexChangeSet {
    pub folders: Vec<FolderId>,
    pub schemes: Vec<SchemeId>,
}

#[derive(Clone)]
pub struct IndexedWorkspace {
    pub workspace: Workspace,
    pub calendar: CalendarIndex,
    pub search: SearchIndex,
    pub channel: ChannelIndex,
}

impl IndexedWorkspace {
    pub fn build(workspace: Workspace) -> Self {
        Self::build_with_expander(workspace, &DefaultExpander)
    }

    pub fn build_with_expander(workspace: Workspace, expander: &dyn OccurrenceExpander) -> Self {
        let calendar = build_calendar_index(&workspace, expander);
        let search = build_search_index(&workspace);
        let channel = build_channel_index(&workspace);
        Self {
            workspace,
            calendar,
            search,
            channel,
        }
    }

    pub fn replace_workspace(&mut self, workspace: Workspace) {
        self.workspace = workspace;
        self.rebuild(&DefaultExpander);
    }

    pub fn apply_changeset(
        &mut self,
        changeset: &IndexChangeSet,
        expander: &dyn OccurrenceExpander,
    ) {
        update_calendar_index(&mut self.calendar, changeset, &self.workspace, expander);
        update_search_index(&mut self.search, changeset, &self.workspace);
        update_channel_index(&mut self.channel, changeset, &self.workspace);
    }

    pub fn rebuild(&mut self, expander: &dyn OccurrenceExpander) {
        self.calendar = build_calendar_index(&self.workspace, expander);
        self.search = build_search_index(&self.workspace);
        self.channel = build_channel_index(&self.workspace);
    }

    pub fn calendar_query(&self) -> crate::query::CalendarQuery<'_> {
        crate::query::CalendarQuery::new(self)
    }

    pub fn search_query<'a>(
        &'a self,
        time_format: knotq_model::TimeFormat,
        options: crate::query::SearchOptions<'a>,
    ) -> crate::query::SearchQuery<'a> {
        crate::query::SearchQuery::new(self, time_format, options)
    }

    pub fn channel_query(&self) -> crate::query::ChannelQuery<'_> {
        crate::query::ChannelQuery::new(self)
    }
}
