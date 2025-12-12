/**
 * Type definitions for Agent module
 */
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ============================================================================
// Chat Message Types
// ============================================================================

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ChatMessage {
    pub role: String,
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<ImageAttachment>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ImageAttachment {
    pub base64: String,
    pub mime_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_uri: Option<String>,
}

#[derive(Serialize, Debug, Clone)]
pub struct ApiChatMessage {
    pub role: String,
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

// ============================================================================
// Tool Call Types
// ============================================================================

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: FunctionCall,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ToolDefinition {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: FunctionDefinition,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FunctionDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

// ============================================================================
// OpenRouter/OpenAI API Types
// ============================================================================

#[derive(Serialize, Debug)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<ApiChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefinition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<ReasoningConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_reasoning: Option<bool>,
    pub stream: bool,
}

#[derive(Serialize, Debug, Clone)]
pub struct ReasoningConfig {
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

// ============================================================================
// Gemini API Types
// ============================================================================

#[derive(Serialize, Deserialize, Debug)]
pub struct GenerateContentRequest {
    pub contents: Vec<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<GeminiTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_instruction: Option<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "generationConfig")]
    pub generation_config: Option<GenerationConfig>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct GenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none", rename = "thinkingConfig")]
    pub thinking_config: Option<ThinkingConfig>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ThinkingConfig {
    #[serde(rename = "includeThoughts")]
    pub include_thoughts: bool,
    #[serde(skip_serializing_if = "Option::is_none", rename = "thinkingBudget")]
    pub thinking_budget: Option<i32>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct GeminiContent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    pub parts: Vec<GeminiPart>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(untagged)]
pub enum GeminiPart {
    Text { text: String },
    FileData {
        #[serde(rename = "fileData")]
        file_data: GeminiFileData,
    },
    FunctionCall {
        #[serde(rename = "functionCall")]
        function_call: GeminiFunctionCall,
    },
    FunctionResponse {
        #[serde(rename = "functionResponse")]
        function_response: GeminiFunctionResponse,
    },
    Thought { thought: bool, text: String },
}

#[derive(Serialize, Deserialize, Debug)]
pub struct GeminiFileData {
    #[serde(rename = "mimeType")]
    pub mime_type: String,
    #[serde(rename = "fileUri")]
    pub file_uri: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct GeminiTool {
    #[serde(rename = "functionDeclarations")]
    pub function_declarations: Vec<FunctionDefinition>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct GeminiFunctionCall {
    pub name: String,
    pub args: Value,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct GeminiFunctionResponse {
    pub name: String,
    pub response: Value,
}

#[derive(Deserialize, Debug)]
pub struct GenerateContentResponse {
    pub candidates: Option<Vec<GeminiCandidate>>,
}

#[derive(Deserialize, Debug)]
pub struct GeminiCandidate {
    pub content: GeminiContent,
}
