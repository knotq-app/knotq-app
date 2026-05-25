use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{CalendarProvider, ItemKind, ItemState, OccurrenceId, OccurrenceState, Recurrence};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Item {
    #[serde(default)]
    pub id: crate::ItemId,
    #[serde(default)]
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub media: Vec<ItemMedia>,
    #[serde(default, skip_serializing_if = "is_default_marker")]
    pub marker: ItemMarker,
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    pub indent: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeats: Option<Recurrence>,
    /// One state slot per stable occurrence identity. Always at least one slot.
    #[serde(default = "default_state", skip_serializing_if = "is_default_state")]
    pub state: Vec<OccurrenceState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external: Option<ExternalItemSource>,
}

impl Item {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            id: crate::ItemId::new(),
            text: text.into(),
            media: Vec::new(),
            marker: ItemMarker::Blank,
            indent: 0,
            start: None,
            end: None,
            available: None,
            repeats: None,
            state: vec![OccurrenceState::default()],
            priority: None,
            external: None,
        }
    }

    pub fn with_indent(mut self, indent: u8) -> Self {
        self.indent = indent;
        self
    }

    pub fn with_start(mut self, dt: DateTime<Utc>) -> Self {
        self.marker = ItemMarker::Checkbox;
        self.start = Some(dt);
        self
    }

    pub fn with_end(mut self, dt: DateTime<Utc>) -> Self {
        self.marker = ItemMarker::Checkbox;
        self.end = Some(dt);
        self
    }

    pub fn with_marker(mut self, marker: ItemMarker) -> Self {
        self.marker = marker;
        self
    }

    pub fn with_repeats(mut self, repeats: Recurrence) -> Self {
        self.marker = ItemMarker::Checkbox;
        self.repeats = Some(repeats);
        self
    }

    pub fn done(mut self) -> Self {
        self.marker = ItemMarker::Checkbox;
        for s in self.state.iter_mut() {
            s.state.progress = -1;
        }
        self
    }

    pub fn kind(&self) -> ItemKind {
        if self.marker != ItemMarker::Checkbox {
            return ItemKind::Procedure;
        }
        match (self.start.is_some(), self.end.is_some()) {
            (true, true) => ItemKind::Event,
            (true, false) => ItemKind::Reminder,
            (false, true) => ItemKind::Assignment,
            (false, false) => ItemKind::Procedure,
        }
    }

    pub fn state_for_occurrence(&self, occurrence: &OccurrenceId) -> ItemState {
        self.state
            .iter()
            .find(|state| &state.occurrence == occurrence)
            .map(|state| state.state)
            .unwrap_or_default()
    }

    pub fn state_for_occurrence_mut(&mut self, occurrence: OccurrenceId) -> &mut ItemState {
        if let Some(index) = self
            .state
            .iter()
            .position(|state| state.occurrence == occurrence)
        {
            return &mut self.state[index].state;
        }
        self.state.push(OccurrenceState {
            occurrence,
            state: ItemState::default(),
        });
        &mut self.state.last_mut().unwrap().state
    }

    pub fn single_state(&self) -> ItemState {
        self.state_for_occurrence(&OccurrenceId::Single)
    }

    pub fn normalize_state(&mut self) {
        self.state
            .retain(|state| state.occurrence == OccurrenceId::Single || !state.state.is_default());
        if self.state.is_empty() {
            self.state.push(OccurrenceState::default());
        }
    }

    pub fn enforce_marker_constraints(&mut self) -> bool {
        let mut changed = false;
        if self.marker == ItemMarker::Checkbox {
            if self.state.is_empty() {
                self.state.push(OccurrenceState::default());
                changed = true;
            }
            self.normalize_state();
            return changed;
        }

        if self.start.take().is_some() {
            changed = true;
        }
        if self.end.take().is_some() {
            changed = true;
        }
        if self.available.take().is_some() {
            changed = true;
        }
        if self.repeats.take().is_some() {
            changed = true;
        }
        let state_has_annotations = self.state.len() != 1
            || self.state.first().is_none_or(|state| {
                state.occurrence != OccurrenceId::Single || !state.state.is_default()
            });
        if state_has_annotations {
            self.state = vec![OccurrenceState::default()];
            changed = true;
        }
        changed
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExternalItemSource {
    pub provider: CalendarProvider,
    pub account_id: String,
    pub calendar_id: String,
    pub event_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemMarker {
    #[default]
    Blank,
    Bullet,
    Numbered,
    Checkbox,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ItemMedia {
    Image {
        asset: Uuid,
        format: ImageAssetFormat,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        width: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        height: Option<u32>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageAssetFormat {
    Png,
    Jpeg,
    Webp,
    Gif,
    Svg,
    Bmp,
    Tiff,
}

impl ImageAssetFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Webp => "webp",
            Self::Gif => "gif",
            Self::Svg => "svg",
            Self::Bmp => "bmp",
            Self::Tiff => "tiff",
        }
    }
}

fn default_state() -> Vec<OccurrenceState> {
    vec![OccurrenceState::default()]
}

fn is_zero_u8(value: &u8) -> bool {
    *value == 0
}

fn is_default_marker(marker: &ItemMarker) -> bool {
    *marker == ItemMarker::Blank
}

fn is_default_state(state: &[OccurrenceState]) -> bool {
    state.len() == 1 && state[0].occurrence == OccurrenceId::Single && state[0].state.is_default()
}
