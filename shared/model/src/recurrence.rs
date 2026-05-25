use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::OccurrenceId;

pub type Recurrence = CalendarRecurrence;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "freq", rename_all = "snake_case")]
pub enum SimpleRecurrence {
    Daily {
        #[serde(default = "default_interval")]
        interval: usize,
        #[serde(default)]
        end: RepeatEnd,
    },
    Weekly {
        #[serde(default = "default_interval")]
        interval: usize,
        weekdays: Vec<RepeatWeekday>,
        #[serde(default)]
        end: RepeatEnd,
    },
    Monthly {
        #[serde(default = "default_interval")]
        interval: usize,
        #[serde(default)]
        end: RepeatEnd,
    },
    Yearly {
        #[serde(default = "default_interval")]
        interval: usize,
        #[serde(default)]
        end: RepeatEnd,
    },
}

impl Default for SimpleRecurrence {
    fn default() -> Self {
        Self::Weekly {
            interval: 1,
            weekdays: Vec::new(),
            end: RepeatEnd::Never,
        }
    }
}

impl SimpleRecurrence {
    pub fn interval(&self) -> usize {
        match self {
            Self::Daily { interval, .. }
            | Self::Weekly { interval, .. }
            | Self::Monthly { interval, .. }
            | Self::Yearly { interval, .. } => (*interval).max(1),
        }
    }

    pub fn repeat_end(&self) -> &RepeatEnd {
        match self {
            Self::Daily { end, .. }
            | Self::Weekly { end, .. }
            | Self::Monthly { end, .. }
            | Self::Yearly { end, .. } => end,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepeatEnd {
    #[default]
    Never,
    Count(usize),
    Until(DateTime<Utc>),
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepeatWeekday {
    Mon,
    Tue,
    Wed,
    Thu,
    Fri,
    Sat,
    Sun,
}

impl RepeatWeekday {
    pub fn abbr(self) -> &'static str {
        match self {
            Self::Mon => "Mon",
            Self::Tue => "Tue",
            Self::Wed => "Wed",
            Self::Thu => "Thu",
            Self::Fri => "Fri",
            Self::Sat => "Sat",
            Self::Sun => "Sun",
        }
    }

    pub fn num_days_from_monday(self) -> u32 {
        match self {
            Self::Mon => 0,
            Self::Tue => 1,
            Self::Wed => 2,
            Self::Thu => 3,
            Self::Fri => 4,
            Self::Sat => 5,
            Self::Sun => 6,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct CalendarRecurrence {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rrules: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rdates: Vec<CalendarDateTime>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exdates: Vec<CalendarDateTime>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overrides: Vec<OccurrenceOverride>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_import: Option<RawCalendarPayload>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OccurrenceOverride {
    pub occurrence: OccurrenceId,
    pub status: OccurrenceOverrideStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available: Option<DateTime<Utc>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OccurrenceOverrideStatus {
    Active,
    Cancelled,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RawCalendarPayload {
    pub content_type: String,
    pub data: String,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CalendarDateTime {
    Date { date: NaiveDate },
    DateTimeUtc { datetime: DateTime<Utc> },
    DateTimeWithZone { local: NaiveDateTime, tzid: String },
}

impl CalendarDateTime {
    pub fn utc(datetime: DateTime<Utc>) -> Self {
        Self::DateTimeUtc { datetime }
    }

    pub fn as_utc_lossy(&self) -> DateTime<Utc> {
        match self {
            Self::Date { date } => {
                DateTime::from_naive_utc_and_offset(date.and_hms_opt(0, 0, 0).unwrap(), Utc)
            }
            Self::DateTimeUtc { datetime } => *datetime,
            Self::DateTimeWithZone { local, .. } => {
                DateTime::from_naive_utc_and_offset(*local, Utc)
            }
        }
    }
}

fn default_interval() -> usize {
    1
}
