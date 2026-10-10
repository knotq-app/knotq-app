use knotq_model::{FolderId, SchemeId, Workspace};
use knotq_rrule::{DefaultExpander, OccurrenceExpander};

use crate::calendar::{build_calendar_index, update_calendar_index, CalendarIndex};
use crate::channel::{build_channel_index, update_channel_index, ChannelIndex};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IndexChangeSet {
    pub folders: Vec<FolderId>,
    pub schemes: Vec<SchemeId>,
}

/// The calendar and channel indexes over a workspace, without the workspace.
///
/// Whoever owns the [`Workspace`] keeps one of these beside it and pairs the
/// two with [`WorkspaceIndex::view`] to query. Queries look everything up by
/// id and skip what is no longer there, so an index that has fallen behind its
/// workspace answers with less rather than with something wrong — but it is
/// the owner's job to rebuild it when the workspace changes.
#[derive(Clone)]
pub struct WorkspaceIndex {
    pub calendar: CalendarIndex,
    pub channel: ChannelIndex,
}

impl WorkspaceIndex {
    pub fn build(workspace: &Workspace) -> Self {
        Self::build_with_expander(workspace, &DefaultExpander)
    }

    pub fn build_with_expander(workspace: &Workspace, expander: &dyn OccurrenceExpander) -> Self {
        Self {
            calendar: build_calendar_index(workspace, expander),
            channel: build_channel_index(workspace),
        }
    }

    pub fn apply_changeset(
        &mut self,
        changeset: &IndexChangeSet,
        workspace: &Workspace,
        expander: &dyn OccurrenceExpander,
    ) {
        update_calendar_index(&mut self.calendar, changeset, workspace, expander);
        update_channel_index(&mut self.channel, changeset, workspace);
    }

    pub fn view<'a>(&'a self, workspace: &'a Workspace) -> IndexedView<'a> {
        IndexedView {
            workspace,
            calendar: &self.calendar,
            channel: &self.channel,
        }
    }
}

/// A workspace paired with its indexes, borrowed: what the queries run against.
#[derive(Clone, Copy)]
pub struct IndexedView<'a> {
    pub workspace: &'a Workspace,
    pub calendar: &'a CalendarIndex,
    pub channel: &'a ChannelIndex,
}

impl<'a> IndexedView<'a> {
    pub fn calendar_query(self) -> crate::query::CalendarQuery<'a> {
        crate::query::CalendarQuery::new(self)
    }

    pub fn search_query(
        self,
        time_format: knotq_model::TimeFormat,
        options: crate::query::SearchOptions<'a>,
    ) -> crate::query::SearchQuery<'a> {
        crate::query::SearchQuery::new(self, time_format, options)
    }

    pub fn channel_query(self) -> crate::query::ChannelQuery<'a> {
        crate::query::ChannelQuery::new(self)
    }
}

/// A workspace together with its indexes, owning both.
///
/// That means a second copy of the workspace for as long as this lives. An
/// owner that already holds the workspace should keep a [`WorkspaceIndex`]
/// beside it instead.
#[derive(Clone)]
pub struct IndexedWorkspace {
    pub workspace: Workspace,
    pub calendar: CalendarIndex,
    pub channel: ChannelIndex,
}

impl IndexedWorkspace {
    pub fn build(workspace: Workspace) -> Self {
        Self::build_with_expander(workspace, &DefaultExpander)
    }

    pub fn build_with_expander(workspace: Workspace, expander: &dyn OccurrenceExpander) -> Self {
        let calendar = build_calendar_index(&workspace, expander);
        let channel = build_channel_index(&workspace);
        Self {
            workspace,
            calendar,
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
        update_channel_index(&mut self.channel, changeset, &self.workspace);
    }

    pub fn rebuild(&mut self, expander: &dyn OccurrenceExpander) {
        self.calendar = build_calendar_index(&self.workspace, expander);
        self.channel = build_channel_index(&self.workspace);
    }

    pub fn view(&self) -> IndexedView<'_> {
        IndexedView {
            workspace: &self.workspace,
            calendar: &self.calendar,
            channel: &self.channel,
        }
    }

    pub fn calendar_query(&self) -> crate::query::CalendarQuery<'_> {
        self.view().calendar_query()
    }

    pub fn search_query<'a>(
        &'a self,
        time_format: knotq_model::TimeFormat,
        options: crate::query::SearchOptions<'a>,
    ) -> crate::query::SearchQuery<'a> {
        self.view().search_query(time_format, options)
    }

    pub fn channel_query(&self) -> crate::query::ChannelQuery<'_> {
        self.view().channel_query()
    }
}
