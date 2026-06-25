//! Unit tests for the CRDT layer. Split into grouped submodules but still a child
//! of the `crdt` module, so each submodule's `use super::super::*` reaches the same
//! internal items the original flat `use super::*` did.

mod helpers;
mod merge;
mod schema_validation;
mod workspace_materialization;
