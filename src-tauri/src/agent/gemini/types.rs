use serde::{Deserialize, Serialize};
use serde_json::Value;

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
    Text {
        text: String,
    },
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
    Thought {
        thought: bool,
        text: String,
    },
}

#[derive(Serialize, Deserialize, Debug)]
pub struct GeminiFileData {
    #[serde(rename = "mimeType")]
    pub mime_type: String,
    #[serde(rename = "fileUri")]
    pub file_uri: String,
}

/// Gemini-specific function definition (excludes OpenAI fields like 'strict')
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct GeminiFunctionDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct GeminiTool {
    #[serde(rename = "functionDeclarations")]
    pub function_declarations: Vec<GeminiFunctionDefinition>,
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

// ============================================================================
// Interactions API Types (Beta)
// ============================================================================

#[derive(Serialize, Debug)]
pub struct InteractionsRequest {
    pub model: String,
    pub input: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_instruction: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<InteractionsTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation_config: Option<InteractionsGenerationConfig>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub store: Option<bool>,
}

#[derive(Serialize, Debug)]
pub struct InteractionsGenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_level: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_summaries: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
}

#[derive(Serialize, Debug)]
#[serde(tag = "type")]
pub enum InteractionsTool {
    #[serde(rename = "function")]
    Function {
        name: String,
        description: String,
        parameters: Value,
    },
}

#[derive(Deserialize, Debug)]
pub struct InteractionsResponse {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub outputs: Option<Vec<InteractionOutput>>,
}

#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
pub enum InteractionOutput {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "thought")]
    Thought {
        #[serde(default)]
        summary: Option<String>,
        #[serde(default)]
        signature: Option<String>,
    },
    #[serde(rename = "function_call")]
    FunctionCall {
        id: String,
        name: String,
        arguments: Value,
    },
    #[serde(rename = "google_search_result")]
    GoogleSearchResult {
        #[serde(default)]
        rendered_content: Option<String>,
    },
}

#[derive(Deserialize, Debug)]
pub struct InteractionStreamEvent {
    pub event_type: String,
    #[serde(default)]
    pub index: Option<usize>,
    #[serde(default)]
    pub delta: Option<InteractionDelta>,
    #[serde(default)]
    pub content: Option<InteractionContentStart>,
    #[serde(default)]
    pub interaction: Option<InteractionsResponse>,
    /// Payload for `step.start` events — contains step type, initial content,
    /// function call name/id, and thought signatures.
    #[serde(default)]
    pub step: Option<serde_json::Value>,
}

#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
pub enum InteractionDelta {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "thought")]
    Thought {
        #[serde(default)]
        thought: Option<String>,
    },
    #[serde(rename = "thought_signature")]
    ThoughtSignature { signature: String },
    #[serde(rename = "thought_summary")]
    ThoughtSummary {
        #[serde(default)]
        content: Option<InteractionDeltaSummaryContent>,
    },
    #[serde(rename = "function_call")]
    FunctionCallDelta {
        id: String,
        name: String,
        arguments: Value,
    },
}

#[derive(Deserialize, Debug)]
pub struct InteractionDeltaSummaryContent {
    #[serde(default)]
    pub text: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct InteractionContentStart {
    #[serde(rename = "type")]
    pub content_type: String,
}

#[derive(Serialize, Debug)]
pub struct InteractionFunctionResult {
    #[serde(rename = "type")]
    pub result_type: String,
    pub name: String,
    pub call_id: String,
    pub result: Value,
}
