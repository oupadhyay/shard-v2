/**
 * Agent module - AI chat agent with Gemini and OpenRouter support.
 *
 * The agent's behaviour is split across small, single-responsibility
 * modules. This file is intentionally thin: it declares the module graph and
 * owns the public re-export surface. Host state (`Agent`, `TurnContext`, and
 * `Agent::new`) lives in [`host`]; every other method on
 * `impl<R: tauri::Runtime> Agent<R>` lives in one of the submodules below.
 *
 * Module layout:
 *   - [`gemini`]       — Gemini / Interactions provider wire types, request
 *                        shaping, and stream parsers.
 *   - [`host`]         — `Agent`, `TurnContext`, and `Agent::new`.
 *   - [`openrouter`]   — OpenAI-compatible request helpers + `to_multimodal_messages`.
 *   - [`provider`]     — Provider-neutral DTOs used at extraction seams.
 *   - [`hash`]         — `calculate_history_hash`.
 *   - [`state`]        — history mutators + SQLite session persistence.
 *   - [`retry`]        — frontend-triggered KaTeX retry path.
 *   - [`youtube_summary`] — long-transcript summarization (chunked).
 *   - [`research`]     — Gemini-based intent classifier for research mode.
 *   - [`tools`]        — `execute_tool` cache wrapper + tool dispatch.
 *   - [`turns`]        — per-turn streaming handlers (Gemini + OpenRouter).
 *   - [`process`]      — `process_message` orchestrator.
 */
mod adapters;
mod gemini;
mod hash;
pub mod hooks;
mod host;
pub(crate) mod openrouter;
mod process;
mod provider;
mod research;
mod retry;
mod state;
mod tools;
mod turns;
mod types;
mod youtube_summary;

pub use gemini::{
    construct_gemini_messages, construct_interactions_input, extract_model_text_from_steps,
    parse_gemini_chunk, parse_interactions_sse_line, process_interactions_event, AgentEvent,
    GeminiCandidate, GeminiContent, GeminiFileData, GeminiFunctionCall,
    GeminiFunctionCallWithSignature, GeminiFunctionDefinition, GeminiFunctionResponse, GeminiPart,
    GeminiTool, GenerateContentRequest, GenerateContentResponse, GenerationConfig,
    InteractionContentStart, InteractionDelta, InteractionDeltaSummaryContent,
    InteractionFunctionResult, InteractionOutput, InteractionStreamEvent,
    InteractionsGenerationConfig, InteractionsRequest, InteractionsResponse, InteractionsTool,
    ThinkingConfig, GEMINI_API_REVISION,
};
pub use host::Agent;
pub(crate) use host::TurnContext;
pub use openrouter::{
    has_images, process_chat_completion_sse_line, send_chat_completion_request, supports_tools,
    to_multimodal_messages, ChatCompletionRequest, OpenAiChatStreamEvent, OpenAiChatStreamState,
    OpenAiChatTransportConfig, ReasoningConfig,
};
pub use provider::{
    ProviderFunctionCall, ProviderFunctionDefinition, ProviderImage, ProviderMessage,
    ProviderToolCall, ProviderToolDefinition,
};
pub use types::{
    ChatMessage, FunctionCall, FunctionDefinition, ImageAttachment, PersistedChatState,
    RetryReason, ToolCall, ToolDefinition,
};

// Phase 6 refactor — re-export the pure helpers so existing callers and
// tests reach them under the same `crate::agent::xxx` paths they used
// before the split.
pub(crate) use gemini::normalize_gemini_schema;
#[cfg(test)]
pub(crate) use hash::calculate_history_hash;
#[cfg(test)]
pub(crate) use youtube_summary::split_transcript_chunks;
