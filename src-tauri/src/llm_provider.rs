//! Compatibility exports for provider-neutral chat DTOs.
//!
//! Canonical definitions live in `shard-provider`; existing host modules keep
//! using `crate::llm_provider` while the repository split proceeds.

pub use shard_provider::chat::*;
