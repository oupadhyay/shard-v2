//! Compatibility re-exports for portable external tools.
//!
//! The Tauri agent host owns lifecycle hooks, caching, frontend events,
//! personas, memory, persistence, and YouTube summarization policy. External
//! API execution and YouTube transcript acquisition/rendering live in
//! `shard-external-tools`.

pub use shard_external_tools::{
    execute_external_tool, fetch_youtube_transcript, ExternalToolConfig, YoutubeProcessConfig,
    YoutubeTranscriptToolOutput,
};
