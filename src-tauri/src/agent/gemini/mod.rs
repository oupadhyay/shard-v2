//! Gemini provider protocol helpers.
//!
//! This module is intentionally host-light: it owns Gemini / Interactions wire
//! types, request-shaping helpers, and stream parsing. The Tauri `Agent` host
//! still owns prompt assembly, memory/persona lookup, DB persistence, tool
//! execution, and frontend events. Keeping that boundary visible here makes a
//! future `shard-llm-providers` crate split mostly mechanical.

mod message;
mod schema;
mod stream;
mod transport;
mod types;

#[cfg(test)]
mod provider_tests;

pub use message::{
    build_generate_content_request, construct_gemini_messages, construct_gemini_tools,
    construct_generate_content_messages, construct_interactions_input,
    construct_interactions_tools, extract_generate_content_text, extract_interactions_text,
    extract_model_text_from_steps, parse_generate_content_completion,
};
#[cfg(test)]
pub(crate) use schema::normalize_gemini_schema;
pub use stream::{
    parse_gemini_chunk, parse_interactions_sse_line, process_interactions_event, AgentEvent,
    GEMINI_API_REVISION,
};
pub use transport::{
    send_generate_content_request, send_interactions_request, send_interactions_stream,
    GeminiGenerateContentTransportConfig, GeminiInteractionsTransportConfig,
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
