//! The in-memory backend now lives in the crate (`knotq_sync::testing`) so every
//! client's production-path fuzzer syncs against the same server model.
pub use knotq_sync::testing::MemoryServer as TestServer;
