use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{Item, ItemId, SchemeId};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Scheme {
    pub id: SchemeId,
    pub name: String,
    pub color_index: u8,
    #[serde(default, skip_serializing_if = "is_false")]
    pub gsync: bool,
    #[serde(default, skip_serializing_if = "SchemeSource::is_local")]
    pub source: SchemeSource,
    pub items: Vec<Item>,
}

impl Scheme {
    pub fn new(name: impl Into<String>, color_index: u8) -> Self {
        Self {
            id: SchemeId::new(),
            name: name.into(),
            color_index,
            gsync: false,
            source: SchemeSource::Local,
            items: Vec::new(),
        }
    }

    pub fn is_read_only(&self) -> bool {
        self.source.is_read_only()
    }

    pub fn item_index(&self, id: ItemId) -> Option<usize> {
        self.items.iter().position(|i| i.id == id)
    }

    pub fn item(&self, id: ItemId) -> Option<&Item> {
        self.items.iter().find(|i| i.id == id)
    }

    pub fn item_mut(&mut self, id: ItemId) -> Option<&mut Item> {
        self.items.iter_mut().find(|i| i.id == id)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SchemeSource {
    #[default]
    Local,
    ImportedCalendar(ImportedCalendarSource),
}

impl SchemeSource {
    pub fn is_local(&self) -> bool {
        matches!(self, Self::Local)
    }

    pub fn is_read_only(&self) -> bool {
        matches!(
            self,
            Self::ImportedCalendar(ImportedCalendarSource {
                read_only: true,
                ..
            })
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ImportedCalendarSource {
    pub provider: CalendarProvider,
    pub account_id: String,
    pub calendar_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync_token: Option<String>,
    #[serde(default = "default_true", skip_serializing_if = "is_false")]
    pub read_only: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_synced_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CalendarProvider {
    Google,
    Apple,
    Ics,
}

fn default_true() -> bool {
    true
}

fn is_false(value: &bool) -> bool {
    !*value
}
