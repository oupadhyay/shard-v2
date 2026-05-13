// Gemini API utilities - message construction and response parsing

use super::types::*;
use serde_json::{json, Value};
use std::collections::HashMap;

/// Api-Revision header value to opt into the new steps schema.
/// Becomes the default on May 26, 2026; legacy removed June 8, 2026.
pub const GEMINI_API_REVISION: &str = "2026-05-20";

/// Extract the first model text from a steps-schema Interactions API response.
/// Concatenates all text items within the first `model_output` step.
pub fn extract_model_text_from_steps(body: &Value) -> Option<String> {
    let steps = body.get("steps")?.as_array()?;
    for step in steps {
        if step.get("type").and_then(|t| t.as_str()) == Some("model_output") {
            let content = step.get("content")?.as_array()?;
            let mut text_parts = String::new();
            for item in content {
                if item.get("type").and_then(|t| t.as_str()) == Some("text") {
                    if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                        text_parts.push_str(t);
                    }
                }
            }
            if !text_parts.is_empty() {
                return Some(text_parts);
            }
        }
    }
    None
}

/// Events emitted during streaming responses
pub enum AgentEvent {
    ResponseChunk(String),
    ReasoningChunk(String),
    ToolCall(GeminiFunctionCallWithSignature),
    /// Interactions API tool call with id, name, and arguments
    InteractionToolCall {
        id: String,
        name: String,
        arguments: Value,
        signature: Option<String>,
    },
}

/// Convert chat history to Gemini API format
pub fn construct_gemini_messages(history: &[ChatMessage]) -> Vec<GeminiContent> {
    // Build a map of tool call IDs to function names for O(1) lookup
    let mut tool_call_names = HashMap::new();
    for msg in history {
        if let Some(tcs) = &msg.tool_calls {
            for tc in tcs {
                tool_call_names.insert(tc.id.clone(), tc.function.name.clone());
            }
        }
    }

    let mut contents: Vec<GeminiContent> = Vec::new();
    let mut i = 0;
    while i < history.len() {
        let msg = &history[i];
        let role = if msg.role == "assistant" {
            "model"
        } else {
            "user"
        };

        if msg.role == "tool" {
            let func_name = msg
                .tool_call_id
                .as_ref()
                .and_then(|id| tool_call_names.get(id))
                .cloned()
                .unwrap_or_else(|| "unknown".to_string());

            let response_json: Value = json!({ "result": msg.content });

            contents.push(GeminiContent {
                role: Some("function".to_string()),
                parts: vec![GeminiPart::FunctionResponse {
                    function_response: GeminiFunctionResponse {
                        name: func_name,
                        response: response_json,
                    },
                }],
            });
        } else {
            let mut parts = Vec::new();
            if let Some(text) = &msg.content {
                let clean_text = if text.trim().starts_with("{") && text.contains("file_data") {
                    if let Ok(parsed) = serde_json::from_str::<Value>(text) {
                        if let Some(parts_arr) = parsed.get("parts").and_then(|p| p.as_array()) {
                            let mut extracted = String::new();
                            for p in parts_arr {
                                if let Some(t) = p.get("text").and_then(|s| s.as_str()) {
                                    extracted.push_str(t);
                                }
                            }
                            extracted
                        } else {
                            text.clone()
                        }
                    } else {
                        text.clone()
                    }
                } else {
                    text.clone()
                };
                if !clean_text.is_empty() {
                    parts.push(GeminiPart::Text { text: clean_text });
                }
            }

            if let Some(images) = &msg.images {
                for img in images {
                    if let Some(uri) = &img.file_uri {
                        parts.push(GeminiPart::FileData {
                            file_data: GeminiFileData {
                                mime_type: img.mime_type.clone(),
                                file_uri: uri.clone(),
                            },
                        });
                    }
                }
            }

            if let Some(tool_calls) = &msg.tool_calls {
                for tc in tool_calls {
                    let args_val: Value =
                        serde_json::from_str(&tc.function.arguments).unwrap_or(json!({}));
                    parts.push(GeminiPart::FunctionCall {
                        function_call: GeminiFunctionCall {
                            name: tc.function.name.clone(),
                            args: args_val,
                        },
                        thought_signature: tc.thought_signature.clone(),
                    });
                }
            }

            if !parts.is_empty() {
                contents.push(GeminiContent {
                    role: Some(role.to_string()),
                    parts,
                });
            }
        }
        i += 1;
    }
    contents
}

/// Parse a Gemini response part and extract events
pub fn parse_gemini_chunk(
    part: GeminiPart,
    full_text: &mut String,
    full_reasoning: &mut String,
    tool_calls: &mut Vec<GeminiFunctionCallWithSignature>,
) -> Vec<AgentEvent> {
    let mut events = Vec::new();
    log::debug!(
        "Gemini part structure: {:?}",
        serde_json::to_string(&part).unwrap_or_default()
    );
    match part {
        GeminiPart::Text { text } => {
            log::debug!("Gemini Text part (length={})", text.len());

            let is_thinking = text.starts_with("**") && text.ends_with("\n\n");

            if is_thinking {
                log::debug!("Detected thinking summary pattern!");
                full_reasoning.push_str(&text);
                events.push(AgentEvent::ReasoningChunk(text));
            } else {
                full_text.push_str(&text);
                events.push(AgentEvent::ResponseChunk(text));
            }
        }
        GeminiPart::Thought { thought, text } => {
            log::debug!("Gemini thought part: thought={}, text={}", thought, text);
            if thought {
                full_reasoning.push_str(&text);
                events.push(AgentEvent::ReasoningChunk(text));
            } else {
                full_text.push_str(&text);
                events.push(AgentEvent::ResponseChunk(text));
            }
        }
        GeminiPart::FunctionCall {
            function_call,
            thought_signature,
        } => {
            let fc = GeminiFunctionCallWithSignature {
                function_call,
                thought_signature,
            };
            tool_calls.push(fc.clone());
            events.push(AgentEvent::ToolCall(fc));
        }
        _ => {
            log::debug!("Gemini other part type");
        }
    }
    events
}

// ============================================================================
// Interactions API utilities
// ============================================================================

/// Convert chat history to Interactions API stateless input format (array of Turn objects)
pub fn construct_interactions_input(history: &[ChatMessage]) -> Value {
    use std::collections::HashMap;
    let mut turns: Vec<Value> = Vec::new();

    // Pre-index: map tool_call_id → function name for O(1) lookup
    let mut call_id_to_name: HashMap<String, String> = HashMap::new();
    for msg in history {
        if msg.role == "assistant" {
            if let Some(tcs) = &msg.tool_calls {
                for tc in tcs {
                    call_id_to_name.insert(tc.id.clone(), tc.function.name.clone());
                }
            }
        }
    }

    for msg in history {
        if msg.role == "tool" {
            let call_id = match &msg.tool_call_id {
                Some(id) if !id.is_empty() => id.clone(),
                _ => {
                    log::warn!("Skipping tool message with missing tool_call_id");
                    continue;
                }
            };
            let func_name = call_id_to_name
                .get(&call_id)
                .cloned()
                .unwrap_or_else(|| "unknown".to_string());

            let result_text = msg.content.clone().unwrap_or_default();
            let result_val = json!([{"type": "text", "text": result_text}]);
            turns.push(json!({
                "role": "user",
                "content": [{
                    "type": "function_result",
                    "name": func_name,
                    "call_id": call_id,
                    "result": result_val,
                }]
            }));
        } else {
            let role = if msg.role == "assistant" {
                "model"
            } else {
                "user"
            };
            let mut content_parts: Vec<Value> = Vec::new();

            // Thought Signature (Required for tool calling)
            if let Some(tool_calls) = &msg.tool_calls {
                if let Some(first_tc) = tool_calls.first() {
                    if let Some(sig) = &first_tc.thought_signature {
                        content_parts.push(json!({
                            "type": "thought",
                            "signature": sig
                        }));
                    }
                }
            }

            if let Some(text) = &msg.content {
                // Clean up old-format JSON content (same as legacy path)
                let clean_text = if text.trim().starts_with('{') && text.contains("file_data") {
                    if let Ok(parsed) = serde_json::from_str::<Value>(text) {
                        if let Some(parts_arr) = parsed.get("parts").and_then(|p| p.as_array()) {
                            let mut extracted = String::new();
                            for p in parts_arr {
                                if let Some(t) = p.get("text").and_then(|s| s.as_str()) {
                                    extracted.push_str(t);
                                }
                            }
                            extracted
                        } else {
                            text.clone()
                        }
                    } else {
                        text.clone()
                    }
                } else {
                    text.clone()
                };
                if !clean_text.is_empty() {
                    content_parts.push(json!({"type": "text", "text": clean_text}));
                }
            }

            // Images: use uri if available (Files API), otherwise inline base64
            if let Some(images) = &msg.images {
                for img in images {
                    if let Some(uri) = &img.file_uri {
                        content_parts.push(json!({
                            "type": "image",
                            "uri": uri,
                            "mime_type": img.mime_type,
                        }));
                    } else {
                        content_parts.push(json!({
                            "type": "image",
                            "data": img.base64,
                            "mime_type": img.mime_type,
                        }));
                    }
                }
            }

            // Tool calls from assistant messages become function_call outputs
            if let Some(tool_calls) = &msg.tool_calls {
                for tc in tool_calls {
                    let args_val: Value =
                        serde_json::from_str(&tc.function.arguments).unwrap_or(json!({}));
                    content_parts.push(json!({
                        "type": "function_call",
                        "id": tc.id,
                        "name": tc.function.name,
                        "arguments": args_val,
                    }));
                }
            }

            if !content_parts.is_empty() {
                turns.push(json!({
                    "role": role,
                    "content": content_parts
                }));
            }
        }
    }

    Value::Array(turns)
}

/// Parse a single SSE line from the Interactions API streaming response.
/// Returns None if the line is not a data line or cannot be parsed.
pub fn parse_interactions_sse_line(line: &str) -> Option<InteractionStreamEvent> {
    let data = line.strip_prefix("data:")?;
    let data = data.trim_start();
    if data.is_empty() || data == "[DONE]" {
        return None;
    }
    match serde_json::from_str::<InteractionStreamEvent>(data) {
        Ok(event) => Some(event),
        Err(e) => {
            log::warn!(
                "Failed to parse Interactions SSE event: {} — raw: {}",
                e,
                data
            );
            None
        }
    }
}

/// Process a stream event from the Interactions API and emit AgentEvents.
pub fn process_interactions_event(
    event: &InteractionStreamEvent,
    full_text: &mut String,
    full_reasoning: &mut String,
) -> Vec<AgentEvent> {
    let mut events = Vec::new();

    match event.event_type.as_str() {
        // Accept both new ("step.delta") and legacy ("content.delta") event names.
        // The delta payload structure is identical — only the SSE event name changed
        // with Api-Revision: 2026-05-20. Legacy name can be removed after June 8, 2026.
        "step.delta" | "content.delta" => {
            if let Some(delta) = &event.delta {
                match delta {
                    InteractionDelta::Text { text } => {
                        full_text.push_str(text);
                        events.push(AgentEvent::ResponseChunk(text.clone()));
                    }
                    InteractionDelta::Thought { thought } => {
                        if let Some(thought_text) = thought {
                            full_reasoning.push_str(thought_text);
                            events.push(AgentEvent::ReasoningChunk(thought_text.clone()));
                        }
                    }
                    InteractionDelta::ThoughtSummary { content } => {
                        if let Some(c) = content {
                            if let Some(text) = &c.text {
                                full_reasoning.push_str(text);
                                events.push(AgentEvent::ReasoningChunk(text.clone()));
                            }
                        }
                    }
                    InteractionDelta::FunctionCallDelta {
                        id,
                        name,
                        arguments,
                    } => {
                        // In the Interactions API stream, signature usually comes in a separate ThoughtSignature event.
                        // We emit them as separate events and let the caller associate them.
                        events.push(AgentEvent::InteractionToolCall {
                            id: id.clone(),
                            name: name.clone(),
                            arguments: arguments.clone(),
                            signature: None,
                        });
                    }
                    InteractionDelta::ThoughtSignature { signature } => {
                        events.push(AgentEvent::InteractionToolCall {
                            id: "".to_string(), // Partial event, signature only
                            name: "".to_string(),
                            arguments: json!(null),
                            signature: Some(signature.clone()),
                        });
                    }
                }
            }
        }
        // step.start can carry initial content (leading text chunk for model_output)
        // and thought signatures. Not processing this drops the first text fragment.
        "step.start" | "content.start" => {
            if let Some(step) = &event.step {
                let step_type = step.get("type").and_then(|t| t.as_str()).unwrap_or("");
                match step_type {
                    "model_output" => {
                        // Extract initial text from content array
                        if let Some(content) = step.get("content").and_then(|c| c.as_array()) {
                            for item in content {
                                if item.get("type").and_then(|t| t.as_str()) == Some("text") {
                                    if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                                        full_text.push_str(text);
                                        events.push(AgentEvent::ResponseChunk(text.to_string()));
                                    }
                                }
                            }
                        }
                    }
                    "thought" => {
                        // Thought step.start carries the signature needed for tool calling
                        if let Some(sig) = step.get("signature").and_then(|s| s.as_str()) {
                            events.push(AgentEvent::InteractionToolCall {
                                id: "".to_string(),
                                name: "".to_string(),
                                arguments: json!(null),
                                signature: Some(sig.to_string()),
                            });
                        }
                    }
                    _ => {
                        log::debug!("Ignoring step.start type: {}", step_type);
                    }
                }
            }
        }
        _ => {
            log::debug!("Interactions event: {}", event.event_type);
        }
    }

    events
}
