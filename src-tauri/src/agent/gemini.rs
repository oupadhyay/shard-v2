// Gemini API utilities - message construction and response parsing

use serde_json::{json, Value};
use super::types::*;

/// Events emitted during streaming responses
pub enum AgentEvent {
    ResponseChunk(String),
    ReasoningChunk(String),
}

/// Convert chat history to Gemini API format
pub fn construct_gemini_messages(history: &[ChatMessage]) -> Vec<GeminiContent> {
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
            let mut func_name = "unknown".to_string();
            for j in (0..i).rev() {
                if history[j].role == "assistant" {
                    if let Some(tcs) = &history[j].tool_calls {
                        for tc in tcs {
                            if Some(&tc.id) == msg.tool_call_id.as_ref() {
                                func_name = tc.function.name.clone();
                                break;
                            }
                        }
                    }
                }
                if func_name != "unknown" {
                    break;
                }
            }

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
        GeminiPart::FunctionCall { function_call, thought_signature } => {
            tool_calls.push(GeminiFunctionCallWithSignature {
                function_call,
                thought_signature,
            });
        }
        _ => {
            log::debug!("Gemini other part type");
        }
    }
    events
}
