use crate::agent::types::ChatMessage;

use serde_json::{json, Value};
use std::collections::HashMap;

use super::types::*;

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

/// Convert chat history to Gemini API format.
///
/// The host-owned [`ChatMessage`] is accepted here for compatibility, but all
/// provider wire structs live in this module. A future split can replace this
/// function's input with a provider-neutral message DTO without moving the
/// Gemini wire code again.
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
                let clean_text = clean_legacy_file_data_text(text);
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

/// Convert chat history to Interactions API stateless input format (array of Step objects).
///
/// The steps-based Interactions API rejects legacy turn-list entries (`role` +
/// `content`) when `Api-Revision: 2026-05-20` is active. For stateless history,
/// send the chronological step timeline instead: user messages become
/// `user_input`, assistant text becomes `model_output`, assistant function calls
/// become `function_call`, and tool messages become `function_result`.
pub fn construct_interactions_input(history: &[ChatMessage]) -> Value {
    let mut steps: Vec<Value> = Vec::new();

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
            steps.push(json!({
                "type": "function_result",
                "name": func_name,
                "call_id": call_id,
                "result": result_val,
            }));
        } else {
            let mut content_parts: Vec<Value> = Vec::new();

            // Thought Signature (Required for tool calling)
            if let Some(tool_calls) = &msg.tool_calls {
                if let Some(first_tc) = tool_calls.first() {
                    if let Some(sig) = &first_tc.thought_signature {
                        steps.push(json!({
                            "type": "thought",
                            "signature": sig
                        }));
                    }
                }
            }

            if let Some(text) = &msg.content {
                let clean_text = clean_legacy_file_data_text(text);
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

            if !content_parts.is_empty() {
                let step_type = if msg.role == "assistant" {
                    "model_output"
                } else {
                    "user_input"
                };
                steps.push(json!({
                    "type": step_type,
                    "content": content_parts
                }));
            }

            // Tool calls from assistant messages become function_call steps.
            if let Some(tool_calls) = &msg.tool_calls {
                for tc in tool_calls {
                    let args_val: Value =
                        serde_json::from_str(&tc.function.arguments).unwrap_or(json!({}));
                    steps.push(json!({
                        "type": "function_call",
                        "id": tc.id,
                        "name": tc.function.name,
                        "arguments": args_val,
                    }));
                }
            }
        }
    }

    Value::Array(steps)
}

fn clean_legacy_file_data_text(text: &str) -> String {
    if text.trim().starts_with('{') && text.contains("file_data") {
        if let Ok(parsed) = serde_json::from_str::<Value>(text) {
            if let Some(parts_arr) = parsed.get("parts").and_then(|p| p.as_array()) {
                let mut extracted = String::new();
                for p in parts_arr {
                    if let Some(t) = p.get("text").and_then(|s| s.as_str()) {
                        extracted.push_str(t);
                    }
                }
                return extracted;
            }
        }
    }

    text.to_string()
}
