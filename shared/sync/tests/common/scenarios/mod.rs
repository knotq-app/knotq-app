//! Hard CRDT scenario functions, backend-agnostic.
//!
//! Each function takes `&mut Harness` and runs one scenario end-to-end.
//! They are called from two thin wrappers:
//!   - `tests/hard_scenarios.rs` — in-memory backend (`Harness::new`)
//!   - `tests/backend_integration.rs` — HTTP backend (`Harness::new_http`)
//!
//! Keep op counts bounded to keep HTTP mode under ~10 min total.
//!
//! Split into grouped submodules by scenario family; every `pub fn scenario_*`
//! is re-exported here so external wrappers keep resolving `scenarios::scenario_*`.

#![allow(dead_code)]

mod carryover;
mod divergence;
mod fuzz;
mod media;
mod notifications_join;
mod race;

// Not every wrapper crate references every scenario family, so the re-exports look
// "unused" in binaries that only call a subset; the harness is a shared helper.
#[allow(unused_imports)]
pub use carryover::*;
#[allow(unused_imports)]
pub use divergence::*;
#[allow(unused_imports)]
pub use fuzz::*;
#[allow(unused_imports)]
pub use media::*;
#[allow(unused_imports)]
pub use notifications_join::*;
#[allow(unused_imports)]
pub use race::*;
