use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::CalendarDateTime;

thread_local! {
    // When set, `Id::new()` mints deterministic UUIDs from this seeded state on the
    // current thread instead of random v4 ones. Off (None) by default, so production
    // is unchanged — this exists so the sync property fuzzer can replay an exact
    // scenario (random ids otherwise make a failing run irreproducible). See
    // [`set_deterministic_id_seed`].
    static DETERMINISTIC_ID_STATE: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

/// Make `Id::new()` deterministic on the current thread: `Some(seed)` mints a
/// reproducible UUID stream seeded by `seed`; `None` restores random v4 ids.
/// Test/fuzz infrastructure only — never call from product code.
pub fn set_deterministic_id_seed(seed: Option<u64>) {
    DETERMINISTIC_ID_STATE.with(|cell| cell.set(seed));
}

fn split_mix_64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn next_id_uuid() -> Uuid {
    DETERMINISTIC_ID_STATE.with(|cell| match cell.get() {
        None => Uuid::new_v4(),
        Some(state) => {
            let mut state = state;
            let hi = split_mix_64(&mut state);
            let lo = split_mix_64(&mut state);
            cell.set(Some(state));
            let mut bytes = [0u8; 16];
            bytes[..8].copy_from_slice(&hi.to_le_bytes());
            bytes[8..].copy_from_slice(&lo.to_le_bytes());
            // Keep RFC-4122 v4 version/variant bits so these read as ordinary random
            // ids (and never collide with the v8 deterministic daily-queue ids).
            bytes[6] = (bytes[6] & 0x0f) | 0x40;
            bytes[8] = (bytes[8] & 0x3f) | 0x80;
            Uuid::from_bytes(bytes)
        }
    })
}

macro_rules! id_newtype {
    ($name:ident) => {
        #[derive(
            Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub Uuid);

        impl $name {
            pub fn new() -> Self {
                Self(next_id_uuid())
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
id_newtype!(RowId);
id_newtype!(ColumnId);
id_newtype!(WorkspaceId);
id_newtype!(ReplicaId);
id_newtype!(OperationId);
id_newtype!(DocumentId);
id_newtype!(ShareId);

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
