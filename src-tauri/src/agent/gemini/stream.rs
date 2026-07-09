use serde_json::{json, Value};

use super::types::*;

/// Api-Revision header value to opt into the new steps schema.
/// Becomes the default on May 26, 2026; legacy removed June 8, 2026.
pub const GEMINI_API_REVISION: &str = "2026-05-20";

/// Events emitted by Gemini stream parsers.
///
/// The name is kept for public compatibility with existing tests and callers;
/// semantically this is now a provider event that the Tauri host maps to UI,
/// persistence, and tool execution side effects.
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

/// Parse a Gemini response part and extract events.
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
