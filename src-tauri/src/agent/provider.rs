use serde::{Deserialize, Serialize};
use serde_json::Value;

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

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ProviderToolDefinition {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: ProviderFunctionDefinition,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ProviderFunctionDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
}
