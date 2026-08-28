//! Host-free Gemini chat protocol helpers.
//!
//! This module owns Gemini / Interactions wire types, request shaping, stream
//! parsing, and raw HTTP transport. The host supplies explicit request and
//! transport configuration and owns all application policy and side effects.

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
pub use schema::normalize_gemini_schema;
pub use stream::{
    parse_gemini_chunk, parse_interactions_sse_line, process_interactions_event, GeminiStreamEvent,
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
