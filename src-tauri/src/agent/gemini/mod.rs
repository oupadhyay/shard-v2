//! Gemini provider protocol helpers.
//!
//! This module is intentionally host-light: it owns Gemini / Interactions wire
//! types, request-shaping helpers, and stream parsing. The Tauri `Agent` host
//! still owns prompt assembly, memory/persona lookup, DB persistence, tool
//! execution, and frontend events. Keeping that boundary visible here makes a
//! future `shard-llm-providers` crate split mostly mechanical.

mod message;
mod stream;
mod types;

pub use message::{
    construct_gemini_messages, construct_interactions_input, extract_model_text_from_steps,
};
pub use stream::{
    parse_gemini_chunk, parse_interactions_sse_line, process_interactions_event, AgentEvent,
    GEMINI_API_REVISION,
};
pub use types::{
    GeminiCandidate, GeminiContent, GeminiFileData, GeminiFunctionCall,
    GeminiFunctionCallWithSignature, GeminiFunctionDefinition, GeminiFunctionResponse, GeminiPart,
    GeminiTool, GenerateContentRequest, GenerateContentResponse, GenerationConfig,
    InteractionContentStart, InteractionDelta, InteractionDeltaSummaryContent,
    InteractionFunctionResult, InteractionOutput, InteractionStreamEvent,
    InteractionsGenerationConfig, InteractionsRequest, InteractionsResponse, InteractionsTool,
    ThinkingConfig,
};
