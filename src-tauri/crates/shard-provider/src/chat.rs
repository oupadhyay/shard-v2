use serde::{Deserialize, Serialize};

pub use shard_tool_api::{
    FunctionDefinition as ProviderFunctionDefinition, ToolDefinition as ProviderToolDefinition,
};

/// Provider-neutral message DTO used at the boundary between the Tauri host
/// and stateless LLM provider protocol code.
///
/// The host owns persisted chat state (`ChatMessage`, cron flags, session DB
/// writes, frontend events). Provider modules should accept this DTO instead
/// of host-owned persisted types so they can move into a standalone provider
/// crate later.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ProviderMessage {
    pub role: String,
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ProviderToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub images: Option<Vec<ProviderImage>>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ProviderImage {
    pub base64: String,
    pub mime_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_uri: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ProviderToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: ProviderFunctionCall,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thought_signature: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ProviderFunctionCall {
    pub name: String,
    pub arguments: String,
}

/// Provider-neutral generation knobs accepted by provider adapters.
///
/// Individual providers may ignore fields they do not support; host code owns
/// policy decisions about which options to set for a given workflow.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProviderGenerationOptions {
    pub temperature: Option<f64>,
    pub max_output_tokens: Option<u32>,
    pub reasoning_effort: Option<String>,
    pub include_reasoning: Option<bool>,
    pub thinking_level: Option<String>,
    pub thinking_summaries: Option<String>,
}

/// Provider-neutral request shape for one-shot or streaming chat-style calls.
///
/// Host code supplies messages, tools, model id, and options without exposing
/// persisted chat types or application state to provider implementations.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderChatRequest {
    pub model: String,
    pub messages: Vec<ProviderMessage>,
    pub tools: Option<Vec<ProviderToolDefinition>>,
    pub tool_choice: Option<String>,
    pub options: ProviderGenerationOptions,
    pub stream: bool,
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct ProviderChatCompletion {
    pub content: Option<String>,
    pub tool_calls: Vec<ProviderToolCall>,
    pub finish_reason: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProviderStreamEvent {
    ReasoningDelta(String),
    ContentDelta(String),
    ToolCallDelta(ProviderToolCall),
}
