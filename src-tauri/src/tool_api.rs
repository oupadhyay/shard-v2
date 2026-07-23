//! Compatibility re-exports for the provider-neutral tool API crate.
//!
//! Existing host modules keep using `crate::tool_api` while the split proceeds,
//! but the canonical definitions live in `shard-tool-api`.

pub use shard_tool_api::*;
