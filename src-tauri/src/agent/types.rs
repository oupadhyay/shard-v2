/**
 * Type definitions for Agent module
 */
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ============================================================================
// Chat Message Types
// ============================================================================

// Helper for backward-compatible deserialization of image/images field
fn deserialize_images<'de, D>(deserializer: D) -> Result<Option<Vec<ImageAttachment>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error;

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum ImageOrImages {
        Single(ImageAttachment),
        Multiple(Vec<ImageAttachment>),
    }

    // First, deserialize into a raw Value to check for both field names
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;

    match value {
        None => Ok(None),
        Some(v) => {
            // Try to parse as ImageOrImages
            serde_json::from_value::<ImageOrImages>(v)
                .map(|ioi| Some(match ioi {
                    ImageOrImages::Single(img) => vec![img],
                    ImageOrImages::Multiple(imgs) => imgs,
                }))
                .map_err(|e| D::Error::custom(format!("Failed to parse images: {}", e)))
        }
    }
}

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
    /// Images attached to the message. Supports backward-compat read from old "image" field.
    #[serde(
        default,
        alias = "image",
        deserialize_with = "deserialize_images",
        skip_serializing_if = "Option::is_none"
    )]
    pub images: Option<Vec<ImageAttachment>>,
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
    /// Thought signature for Gemini 3 models - required for function calling
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thought_signature: Option<String>,
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
    /// Required by Cerebras for proper tool calling
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
}

// ============================================================================
// Auto-Retry Types
// ============================================================================

/// Reason for auto-retrying a response
#[derive(Debug, Clone)]
pub enum RetryReason {
    /// Model produced reasoning but no user-facing content
    EmptyResponse,
    /// Frontend detected KaTeX parse errors in the response
    MalformedLatex { errors: Vec<String> },
}

impl RetryReason {
    /// Get the retry hint to inject as a system message
    pub fn get_hint(&self) -> String {
        match self {
            RetryReason::EmptyResponse => {
                "[RETRY HINT] Your previous response contained only internal reasoning \
                but no user-facing content. Please provide a complete, visible answer to the user's question."
                    .to_string()
            }
            RetryReason::MalformedLatex { errors } => {
                format!(
                    "[RETRY HINT] Your previous response had LaTeX rendering errors:\n{}\n\n\
                    Please rewrite using correct LaTeX syntax:\n\
                    - Use single $ for inline math: $x^2$\n\
                    - Use $$ on separate lines for display math (not inline)\n\
                    - Ensure all delimiters are properly paired\n\
                    - Avoid nested $ symbols like $a$b$ - use $a$ $b$ instead",
                    errors.join("\n")
                )
            }
        }
    }
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
        #[serde(rename = "thoughtSignature", skip_serializing_if = "Option::is_none")]
        thought_signature: Option<String>,
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

/// Function call paired with its thought signature for Gemini 3 models
#[derive(Debug, Clone)]
pub struct GeminiFunctionCallWithSignature {
    pub function_call: GeminiFunctionCall,
    pub thought_signature: Option<String>,
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
