use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::CalendarDateTime;

macro_rules! id_newtype {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub Uuid);

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(f)
            }
        }

        impl std::str::FromStr for $name {
            type Err = uuid::Error;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(s).map(Self)
            }
        }
    };
}

id_newtype!(FolderId);
id_newtype!(SchemeId);
id_newtype!(ItemId);

#[derive(Clone, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OccurrenceId {
    #[default]
    Single,
    Recurring {
        original_start: CalendarDateTime,
    },
}

impl OccurrenceId {
    pub fn is_single(&self) -> bool {
        matches!(self, Self::Single)
    }

    pub fn recurring_utc(original_start: chrono::DateTime<chrono::Utc>) -> Self {
        Self::Recurring {
            original_start: CalendarDateTime::DateTimeUtc {
                datetime: original_start,
            },
        }
    }
}
