// OpenRouter API utilities - message conversion helpers
// Supports both text-only and multimodal (text + image) messages

use serde::Serialize;

#[cfg(test)]
#[path = "openrouter/provider_tests.rs"]
mod provider_tests;

use super::provider::{
    ProviderFunctionCall, ProviderMessage, ProviderToolCall, ProviderToolDefinition,
};

#[derive(Serialize, Debug)]
pub struct ChatCompletionRequest<M: Serialize> {
    pub model: String,
    pub messages: M,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ProviderToolDefinition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<ReasoningConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_reasoning: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    pub stream: bool,
}

#[derive(Serialize, Debug, Clone)]
pub struct ReasoningConfig {
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct OpenAiChatStreamState {
    pub content: String,
    pub reasoning: String,
    pub tool_calls: Vec<ProviderToolCall>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OpenAiChatStreamEvent {
    ReasoningDelta(String),
    ContentDelta(String),
    ToolCallDelta(ProviderToolCall),
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct OpenAiChatCompletion {
    pub content: Option<String>,
    pub tool_calls: Vec<ProviderToolCall>,
    pub finish_reason: String,
}

#[derive(Debug, Clone)]
pub struct OpenAiChatTransportConfig {
    pub endpoint_url: String,
    pub auth_token: String,
}

pub async fn send_chat_completion_request<B: serde::Serialize + ?Sized>(
    client: &reqwest::Client,
    config: &OpenAiChatTransportConfig,
    request: &B,
) -> Result<reqwest::Response, reqwest::Error> {
    client
        .post(&config.endpoint_url)
        .header("Authorization", format!("Bearer {}", config.auth_token))
        .header("Content-Type", "application/json")
        .header("User-Agent", "rust-reqwest/0.12")
        .json(request)
        .send()
        .await
}

/// Convert chat messages to multimodal API format with image support
/// Returns a JSON Value that can be used directly in the request
pub fn to_multimodal_messages(messages: &[ProviderMessage]) -> Vec<serde_json::Value> {
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
pub fn has_images(messages: &[ProviderMessage]) -> bool {
    messages.iter().any(|msg| {
        msg.images
            .as_ref()
            .map(|imgs| !imgs.is_empty())
            .unwrap_or(false)
    })
}

pub fn process_chat_completion_sse_line(
    line: &str,
    state: &mut OpenAiChatStreamState,
) -> Vec<OpenAiChatStreamEvent> {
    let Some(json_str) = line.trim().strip_prefix("data: ") else {
        return Vec::new();
    };

    if json_str == "[DONE]" {
        return Vec::new();
    }

    let Ok(json) = serde_json::from_str::<serde_json::Value>(json_str) else {
        return Vec::new();
    };

    let Some(choice) = json
        .get("choices")
        .and_then(|choices| choices.as_array())
        .and_then(|choices| choices.first())
    else {
        return Vec::new();
    };

    let delta = &choice["delta"];
    let mut events = Vec::new();

    if let Some(reasoning) = delta
        .get("reasoning")
        .and_then(|reasoning| reasoning.as_str())
    {
        state.reasoning.push_str(reasoning);
        events.push(OpenAiChatStreamEvent::ReasoningDelta(reasoning.to_string()));
    }

    if let Some(content) = delta.get("content").and_then(|content| content.as_str()) {
        state.content.push_str(content);
        events.push(OpenAiChatStreamEvent::ContentDelta(content.to_string()));
    }

    if let Some(tool_calls_arr) = delta.get("tool_calls").and_then(|calls| calls.as_array()) {
        for tool_call_json in tool_calls_arr {
            let index = tool_call_json["index"].as_u64().unwrap_or(0) as usize;
            if index >= state.tool_calls.len() {
                state.tool_calls.resize(
                    index + 1,
                    ProviderToolCall {
                        id: String::new(),
                        tool_type: "function".to_string(),
                        function: ProviderFunctionCall {
                            name: String::new(),
                            arguments: String::new(),
                        },
                        thought_signature: None,
                    },
                );
            }

            let target = &mut state.tool_calls[index];
            if let Some(id) = tool_call_json["id"].as_str() {
                target.id = id.to_string();
            }
            if let Some(tool_type) = tool_call_json["type"].as_str() {
                target.tool_type = tool_type.to_string();
            }
            if let Some(func) = tool_call_json.get("function") {
                if let Some(name) = func["name"].as_str() {
                    target.function.name.push_str(name);
                }
                if let Some(args) = func["arguments"].as_str() {
                    target.function.arguments.push_str(args);
                }
            }

            if !target.function.name.is_empty() {
                events.push(OpenAiChatStreamEvent::ToolCallDelta(target.clone()));
            }
        }
    }

    events
}

pub fn extract_chat_completion_text(body: &serde_json::Value) -> Option<String> {
    body.get("choices")
        .and_then(|choices| choices.as_array())
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .and_then(|content| content.as_str())
        .map(|content| content.to_string())
}

pub fn parse_chat_completion(body: &serde_json::Value) -> Result<OpenAiChatCompletion, String> {
    let choice = body
        .get("choices")
        .and_then(|choices| choices.as_array())
        .and_then(|choices| choices.first())
        .ok_or_else(|| "No choices in chat completion response".to_string())?;

    let message = choice
        .get("message")
        .ok_or_else(|| "No message in chat completion choice".to_string())?;
    let finish_reason = choice
        .get("finish_reason")
        .and_then(|finish_reason| finish_reason.as_str())
        .unwrap_or("stop")
        .to_string();
    let content = message
        .get("content")
        .and_then(|content| content.as_str())
        .map(|content| content.to_string());

    let mut tool_calls = Vec::new();
    if let Some(tool_call_values) = message.get("tool_calls").and_then(|calls| calls.as_array()) {
        for tool_call_value in tool_call_values {
            let id = tool_call_value
                .get("id")
                .and_then(|id| id.as_str())
                .unwrap_or("unknown")
                .to_string();
            let tool_type = tool_call_value
                .get("type")
                .and_then(|tool_type| tool_type.as_str())
                .unwrap_or("function")
                .to_string();

            if let Some(function) = tool_call_value.get("function") {
                let name = function
                    .get("name")
                    .and_then(|name| name.as_str())
                    .unwrap_or("")
                    .to_string();
                let arguments = function
                    .get("arguments")
                    .map(|arguments| {
                        if let Some(arguments) = arguments.as_str() {
                            arguments.to_string()
                        } else {
                            arguments.to_string()
                        }
                    })
                    .unwrap_or_default();

                tool_calls.push(ProviderToolCall {
                    id,
                    tool_type,
                    function: ProviderFunctionCall { name, arguments },
                    thought_signature: None,
                });
            }
        }
    }

    Ok(OpenAiChatCompletion {
        content,
        tool_calls,
        finish_reason,
    })
}

/// Check if a model supports tool calling
pub fn supports_tools(model: &str) -> bool {
    !model.contains("olmo-3.1-32b-think")
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    #[test]
    fn process_chat_completion_sse_line_accumulates_content_reasoning_and_tool_calls() {
        let mut state = OpenAiChatStreamState::default();

        let events = process_chat_completion_sse_line(
            r#"data: {"choices":[{"delta":{"reasoning":"think ","content":"Hel"}}]}"#,
            &mut state,
        );
        assert_eq!(
            events,
            vec![
                OpenAiChatStreamEvent::ReasoningDelta("think ".to_string()),
                OpenAiChatStreamEvent::ContentDelta("Hel".to_string())
            ]
        );

        let events = process_chat_completion_sse_line(
            r#"data: {"choices":[{"delta":{"content":"lo","tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"lookup","arguments":"{\"q\""}}]}}]}"#,
            &mut state,
        );
        assert_eq!(state.content, "Hello");
        assert_eq!(state.reasoning, "think ");
        assert_eq!(state.tool_calls[0].id, "call_1");
        assert_eq!(state.tool_calls[0].function.name, "lookup");
        assert_eq!(state.tool_calls[0].function.arguments, "{\"q\"");
        assert_eq!(events.len(), 2);
        assert_eq!(
            events[0],
            OpenAiChatStreamEvent::ContentDelta("lo".to_string())
        );

        let events = process_chat_completion_sse_line(
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":":\"rust\"}"}}]}}]}"#,
            &mut state,
        );
        assert_eq!(events.len(), 1);
        assert_eq!(state.tool_calls[0].function.arguments, "{\"q\":\"rust\"}");
        match &events[0] {
            OpenAiChatStreamEvent::ToolCallDelta(call) => {
                assert_eq!(call.function.name, "lookup");
                assert_eq!(call.function.arguments, "{\"q\":\"rust\"}");
            }
            other => panic!("expected tool-call delta, got {other:?}"),
        }
    }

    #[test]
    fn process_chat_completion_sse_line_ignores_done_invalid_and_non_data_lines() {
        let mut state = OpenAiChatStreamState::default();

        assert!(process_chat_completion_sse_line("event: message", &mut state).is_empty());
        assert!(process_chat_completion_sse_line("data: [DONE]", &mut state).is_empty());
        assert!(process_chat_completion_sse_line("data: not-json", &mut state).is_empty());
        assert!(state.content.is_empty());
        assert!(state.reasoning.is_empty());
        assert!(state.tool_calls.is_empty());
    }

    #[tokio::test]
    async fn send_chat_completion_request_uses_configured_url_and_auth_headers() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(header("authorization", "Bearer test-openrouter"))
            .and(header("content-type", "application/json"))
            .and(header("user-agent", "rust-reqwest/0.12"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
            .expect(1)
            .mount(&server)
            .await;

        let config = OpenAiChatTransportConfig {
            endpoint_url: format!("{}/chat/completions", server.uri()),
            auth_token: "test-openrouter".to_string(),
        };

        let response = send_chat_completion_request(
            &reqwest::Client::new(),
            &config,
            &json!({"model": "test", "messages": [], "stream": true}),
        )
        .await
        .expect("request should succeed");

        assert!(response.status().is_success());
    }
}
