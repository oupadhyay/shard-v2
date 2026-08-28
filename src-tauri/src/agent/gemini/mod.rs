//! Compatibility exports for Gemini chat protocol code.
//!
//! Wire DTOs, request shaping, stream parsing, and HTTP transport live in
//! `shard-provider`; Shard owns orchestration, state, tools, and UI events.

pub use shard_provider::gemini_chat::*;

/// Compatibility name for existing host call sites and tests.
pub type AgentEvent = GeminiStreamEvent;
