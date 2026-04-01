// OpenRouter API utilities - message conversion helpers
// Supports both text-only and multimodal (text + image) messages

use super::types::*;

/// Convert chat messages to multimodal API format with image support
/// Returns a JSON Value that can be used directly in the request
pub fn to_multimodal_messages(messages: &[ChatMessage]) -> Vec<serde_json::Value> {
    messages
        .iter()
        .map(|msg| {
            // Check if this message has images
            if let Some(images) = &msg.images {
                if !images.is_empty() {
                    // Build multimodal content parts
                    let mut parts: Vec<serde_json::Value> = Vec::new();

                    // Add text content first if present
                    if let Some(text) = &msg.content {
                        if !text.is_empty() {
                            parts.push(serde_json::json!({
                                "type": "text",
                                "text": text
                            }));
                        }
                    }

                    // Add image parts
                    for img in images {
                        let data_uri = format!("data:{};base64,{}", img.mime_type, img.base64);
                        parts.push(serde_json::json!({
                            "type": "image_url",
                            "image_url": {
                                "url": data_uri
                            }
                        }));
                    }

                    let mut message = serde_json::json!({
                        "role": msg.role,
                        "content": parts
                    });

                    // Add optional fields
                    if let Some(tool_calls) = &msg.tool_calls {
                        message["tool_calls"] =
                            serde_json::to_value(tool_calls).unwrap_or_default();
                    }
                    if let Some(tool_call_id) = &msg.tool_call_id {
                        message["tool_call_id"] = serde_json::json!(tool_call_id);
                    }

                    return message;
                }
            }

            // No images - return regular text message
            let mut message = serde_json::json!({
                "role": msg.role
            });

            // Always include content key (null when absent) — some providers
            // require `content: null` on assistant messages with tool_calls.
            match &msg.content {
                Some(content) => message["content"] = serde_json::json!(content),
                None => message["content"] = serde_json::Value::Null,
            }
            if let Some(tool_calls) = &msg.tool_calls {
                message["tool_calls"] = serde_json::to_value(tool_calls).unwrap_or_default();
            }
            if let Some(tool_call_id) = &msg.tool_call_id {
                message["tool_call_id"] = serde_json::json!(tool_call_id);
            }

            message
        })
        .collect()
}

/// Check if any message in the conversation contains images
pub fn has_images(messages: &[ChatMessage]) -> bool {
    messages.iter().any(|msg| {
        msg.images
            .as_ref()
            .map(|imgs| !imgs.is_empty())
            .unwrap_or(false)
    })
}

/// Check if a model supports tool calling
pub fn supports_tools(model: &str) -> bool {
    !model.contains("o1-")
        && !model.contains("o3-")
        && !model.contains("deepseek-reasoner")
        && !model.contains("olmo-3.1-32b-think")
}
