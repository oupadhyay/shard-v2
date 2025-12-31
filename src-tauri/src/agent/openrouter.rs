// OpenRouter API utilities - message conversion helpers
// These helper functions are available for future use

#![allow(dead_code)]

use super::types::*;

// Convert chat messages to OpenRouter/OpenAI API format
pub fn to_api_messages(messages: &[ChatMessage]) -> Vec<ApiChatMessage> {
    messages
        .iter()
        .map(|msg| ApiChatMessage {
            role: msg.role.clone(),
            content: msg.content.clone(),
            tool_calls: msg.tool_calls.clone(),
            tool_call_id: msg.tool_call_id.clone(),
        })
        .collect()
}

// Check if a model supports tool calling
pub fn supports_tools(model: &str) -> bool {
    !model.contains("olmo-3.1-32b-think")
}
