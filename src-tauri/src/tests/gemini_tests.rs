#[cfg(test)]
mod tests {
    use crate::agent::{
        construct_gemini_messages, construct_interactions_input, parse_interactions_sse_line,
        process_interactions_event, ChatMessage, FunctionCall, GeminiPart, ImageAttachment,
        InteractionDelta, InteractionOutput, InteractionStreamEvent, ToolCall,
    };
    use serde_json::json;

    // ========================================================================
    // Legacy Gemini API tests (retained for backward compat)
    // ========================================================================

    #[test]
    fn test_deserialize_gemini_function_call() {
        let json_data = json!({
            "functionCall": {
                "name": "get_weather",
                "args": {
                    "location": "San Francisco, CA"
                }
            }
        });

        let part: GeminiPart =
            serde_json::from_value(json_data).expect("Failed to deserialize FunctionCall");

        if let GeminiPart::FunctionCall {
            function_call,
            thought_signature: _,
        } = part
        {
            assert_eq!(function_call.name, "get_weather");
            assert_eq!(function_call.args["location"], "San Francisco, CA");
        } else {
            panic!("Expected FunctionCall variant");
        }
    }

    #[test]
    fn test_deserialize_gemini_text() {
        let json_data = json!({
            "text": "Hello world"
        });

        let part: GeminiPart =
            serde_json::from_value(json_data).expect("Failed to deserialize Text");

        if let GeminiPart::Text { text } = part {
            assert_eq!(text, "Hello world");
        } else {
            panic!("Expected Text variant");
        }
    }

    #[test]
    fn test_deserialize_gemini_file_data() {
        let json_data = json!({
            "fileData": {
                "mimeType": "image/png",
                "fileUri": "https://example.com/image.png"
            }
        });

        let part: GeminiPart =
            serde_json::from_value(json_data).expect("Failed to deserialize FileData");

        if let GeminiPart::FileData { file_data } = part {
            assert_eq!(file_data.mime_type, "image/png");
            assert_eq!(file_data.file_uri, "https://example.com/image.png");
        } else {
            panic!("Expected FileData variant");
        }
    }

    #[test]
    fn test_deserialize_gemini_function_response() {
        let json_data = json!({
            "functionResponse": {
                "name": "get_weather",
                "response": {
                    "result": "Sunny, 25C"
                }
            }
        });

        let part: GeminiPart =
            serde_json::from_value(json_data).expect("Failed to deserialize FunctionResponse");

        if let GeminiPart::FunctionResponse { function_response } = part {
            assert_eq!(function_response.name, "get_weather");
            assert_eq!(function_response.response["result"], "Sunny, 25C");
        } else {
            panic!("Expected FunctionResponse variant");
        }
    }

    #[test]
    fn test_construct_gemini_messages_basic() {
        let history = vec![
            ChatMessage {
                role: "user".to_string(),
                content: Some("Hello".to_string()),
                reasoning: None,
                tool_calls: None,
                tool_call_id: None,
                is_cron: None,
                images: None,
            },
            ChatMessage {
                role: "assistant".to_string(),
                content: Some("Hi there!".to_string()),
                reasoning: None,
                tool_calls: None,
                tool_call_id: None,
                is_cron: None,
                images: None,
            },
        ];

        let result = construct_gemini_messages(&history);

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].role, Some("user".to_string()));
        if let GeminiPart::Text { text } = &result[0].parts[0] {
            assert_eq!(text, "Hello");
        } else {
            panic!("Expected text part");
        }

        assert_eq!(result[1].role, Some("model".to_string()));
        if let GeminiPart::Text { text } = &result[1].parts[0] {
            assert_eq!(text, "Hi there!");
        } else {
            panic!("Expected text part");
        }
    }

    #[test]
    fn test_construct_gemini_messages_with_images() {
        let history = vec![ChatMessage {
            role: "user".to_string(),
            content: Some("What is this?".to_string()),
            reasoning: None,
            tool_calls: None,
            tool_call_id: None,
            is_cron: None,
            images: Some(vec![ImageAttachment {
                base64: "base64data".to_string(),
                mime_type: "image/png".to_string(),
                file_uri: Some("https://example.com/image.png".to_string()),
            }]),
        }];

        let result = construct_gemini_messages(&history);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].parts.len(), 2);

        if let GeminiPart::Text { text } = &result[0].parts[0] {
            assert_eq!(text, "What is this?");
        } else {
            panic!("Expected text part");
        }

        if let GeminiPart::FileData { file_data } = &result[0].parts[1] {
            assert_eq!(file_data.mime_type, "image/png");
            assert_eq!(file_data.file_uri, "https://example.com/image.png");
        } else {
            panic!("Expected FileData part");
        }
    }

    #[test]
    fn test_construct_gemini_messages_with_tool_calls() {
        let history = vec![ChatMessage {
            role: "assistant".to_string(),
            content: Some("Let me check the weather.".to_string()),
            reasoning: None,
            tool_calls: Some(vec![ToolCall {
                id: "call_1".to_string(),
                tool_type: "function".to_string(),
                function: FunctionCall {
                    name: "get_weather".to_string(),
                    arguments: "{\"location\": \"London\"}".to_string(),
                },
                thought_signature: Some("sig123".to_string()),
            }]),
            tool_call_id: None,
            is_cron: None,
            images: None,
        }];

        let result = construct_gemini_messages(&history);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].role, Some("model".to_string()));
        assert_eq!(result[0].parts.len(), 2);

        if let GeminiPart::Text { text } = &result[0].parts[0] {
            assert_eq!(text, "Let me check the weather.");
        } else {
            panic!("Expected text part");
        }

        if let GeminiPart::FunctionCall {
            function_call,
            thought_signature,
        } = &result[0].parts[1]
        {
            assert_eq!(function_call.name, "get_weather");
            assert_eq!(function_call.args["location"], "London");
            assert_eq!(thought_signature.as_deref(), Some("sig123"));
        } else {
            panic!("Expected FunctionCall part");
        }
    }

    #[test]
    fn test_construct_gemini_messages_with_tool_response() {
        let history = vec![
            ChatMessage {
                role: "assistant".to_string(),
                content: None,
                reasoning: None,
                tool_calls: Some(vec![ToolCall {
                    id: "call_1".to_string(),
                    tool_type: "function".to_string(),
                    function: FunctionCall {
                        name: "get_weather".to_string(),
                        arguments: "{\"location\": \"London\"}".to_string(),
                    },
                    thought_signature: None,
                }]),
                tool_call_id: None,
                is_cron: None,
                images: None,
            },
            ChatMessage {
                role: "tool".to_string(),
                content: Some("Sunny, 20C".to_string()),
                reasoning: None,
                tool_calls: None,
                tool_call_id: Some("call_1".to_string()),
                is_cron: None,
                images: None,
            },
        ];

        let result = construct_gemini_messages(&history);

        assert_eq!(result.len(), 2);

        // Assistant/model part
        assert_eq!(result[0].role, Some("model".to_string()));

        // Tool/function part
        assert_eq!(result[1].role, Some("function".to_string()));
        if let GeminiPart::FunctionResponse { function_response } = &result[1].parts[0] {
            assert_eq!(function_response.name, "get_weather"); // Found from previous assistant message
            assert_eq!(function_response.response["result"], "Sunny, 20C");
        } else {
            panic!("Expected FunctionResponse part");
        }
    }

    #[test]
    fn test_construct_gemini_messages_tool_response_fallback() {
        let history = vec![ChatMessage {
            role: "tool".to_string(),
            content: Some("No context".to_string()),
            reasoning: None,
            tool_calls: None,
            tool_call_id: Some("missing_call_id".to_string()),
            is_cron: None,
            images: None,
        }];

        let result = construct_gemini_messages(&history);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].role, Some("function".to_string()));
        if let GeminiPart::FunctionResponse { function_response } = &result[0].parts[0] {
            assert_eq!(function_response.name, "unknown");
        } else {
            panic!("Expected FunctionResponse part");
        }
    }

    #[test]
    fn test_construct_gemini_messages_json_cleaning() {
        let json_content = json!({
            "parts": [
                { "text": "Extracted " },
                { "text": "text" }
            ],
            "file_data": {} // Triggers the cleaning logic
        })
        .to_string();

        let history = vec![ChatMessage {
            role: "user".to_string(),
            content: Some(json_content),
            reasoning: None,
            tool_calls: None,
            tool_call_id: None,
            is_cron: None,
            images: None,
        }];

        let result = construct_gemini_messages(&history);

        assert_eq!(result.len(), 1);
        if let GeminiPart::Text { text } = &result[0].parts[0] {
            assert_eq!(text, "Extracted text");
        } else {
            panic!("Expected text part");
        }
    }

    #[test]
    fn test_construct_gemini_messages_empty_content() {
        let history = vec![ChatMessage {
            role: "user".to_string(),
            content: None,
            reasoning: None,
            tool_calls: None,
            tool_call_id: None,
            is_cron: None,
            images: None,
        }];

        let result = construct_gemini_messages(&history);

        // Should be empty because content is None and no images/tool_calls
        assert_eq!(result.len(), 0);
    }

    // ========================================================================
    // Interactions API struct deserialization tests
    // ========================================================================

    #[test]
    fn test_deserialize_interaction_function_call() {
        let json_data = json!({
            "type": "function_call",
            "id": "gth23981",
            "name": "get_weather",
            "arguments": {
                "location": "Boston, MA"
            }
        });

        let output: InteractionOutput =
            serde_json::from_value(json_data).expect("Failed to deserialize InteractionOutput");

        if let InteractionOutput::FunctionCall {
            id,
            name,
            arguments,
        } = output
        {
            assert_eq!(id, "gth23981");
            assert_eq!(name, "get_weather");
            assert_eq!(arguments["location"], "Boston, MA");
        } else {
            panic!("Expected FunctionCall variant");
        }
    }

    #[test]
    fn test_deserialize_interaction_text() {
        let json_data = json!({
            "type": "text",
            "text": "Hello from Interactions API"
        });

        let output: InteractionOutput =
            serde_json::from_value(json_data).expect("Failed to deserialize InteractionOutput");

        if let InteractionOutput::Text { text } = output {
            assert_eq!(text, "Hello from Interactions API");
        } else {
            panic!("Expected Text variant");
        }
    }

    #[test]
    fn test_deserialize_interaction_thought() {
        let json_data = json!({
            "type": "thought",
            "summary": "Let me think about this...",
            "signature": "abc123sig"
        });

        let output: InteractionOutput =
            serde_json::from_value(json_data).expect("Failed to deserialize InteractionOutput");

        if let InteractionOutput::Thought { summary, signature } = output {
            assert_eq!(summary.as_deref(), Some("Let me think about this..."));
            assert_eq!(signature.as_deref(), Some("abc123sig"));
        } else {
            panic!("Expected Thought variant");
        }
    }

    #[test]
    fn test_deserialize_interaction_thought_signature_only() {
        let json_data = json!({
            "type": "thought",
            "signature": "abc123sig"
        });

        let output: InteractionOutput =
            serde_json::from_value(json_data).expect("Failed to deserialize InteractionOutput");

        if let InteractionOutput::Thought { summary, signature } = output {
            assert!(summary.is_none());
            assert_eq!(signature.as_deref(), Some("abc123sig"));
        } else {
            panic!("Expected Thought variant");
        }
    }

    #[test]
    fn test_deserialize_interaction_google_search() {
        let json_data = json!({
            "type": "google_search_result",
            "rendered_content": "<div>Search results</div>"
        });

        let output: InteractionOutput =
            serde_json::from_value(json_data).expect("Failed to deserialize InteractionOutput");

        if let InteractionOutput::GoogleSearchResult { rendered_content } = output {
            assert_eq!(
                rendered_content.as_deref(),
                Some("<div>Search results</div>")
            );
        } else {
            panic!("Expected GoogleSearchResult variant");
        }
    }

    // ========================================================================
    // Interactions API request serialization tests
    // ========================================================================

    #[test]
    fn test_construct_interaction_request_basic() {
        let history = vec![
            ChatMessage {
                role: "user".to_string(),
                content: Some("Hello".to_string()),
                reasoning: None,
                tool_calls: None,
                tool_call_id: None,
                is_cron: None,
                images: None,
            },
            ChatMessage {
                role: "assistant".to_string(),
                content: Some("Hi there!".to_string()),
                reasoning: None,
                tool_calls: None,
                tool_call_id: None,
                is_cron: None,
                images: None,
            },
        ];

        let input = construct_interactions_input(&history);
        let turns = input.as_array().expect("Expected array");

        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0]["role"], "user");
        assert_eq!(turns[0]["content"][0]["type"], "text");
        assert_eq!(turns[0]["content"][0]["text"], "Hello");

        assert_eq!(turns[1]["role"], "model");
        assert_eq!(turns[1]["content"][0]["type"], "text");
        assert_eq!(turns[1]["content"][0]["text"], "Hi there!");
    }

    #[test]
    fn test_construct_interaction_request_with_images() {
        let history = vec![ChatMessage {
            role: "user".to_string(),
            content: Some("What is this?".to_string()),
            reasoning: None,
            tool_calls: None,
            tool_call_id: None,
            is_cron: None,
            images: Some(vec![ImageAttachment {
                base64: "base64data".to_string(),
                mime_type: "image/png".to_string(),
                file_uri: Some("https://files.example.com/image.png".to_string()),
            }]),
        }];

        let input = construct_interactions_input(&history);
        let turns = input.as_array().expect("Expected array");

        assert_eq!(turns.len(), 1);
        let content = turns[0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);

        // Text part
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "What is this?");

        // Image part — uses uri when file_uri is present
        assert_eq!(content[1]["type"], "image");
        assert_eq!(content[1]["uri"], "https://files.example.com/image.png");
        assert_eq!(content[1]["mime_type"], "image/png");
    }

    #[test]
    fn test_construct_interaction_request_with_inline_image() {
        let history = vec![ChatMessage {
            role: "user".to_string(),
            content: Some("Describe this".to_string()),
            reasoning: None,
            tool_calls: None,
            tool_call_id: None,
            is_cron: None,
            images: Some(vec![ImageAttachment {
                base64: "iVBORw0KGgo=".to_string(),
                mime_type: "image/png".to_string(),
                file_uri: None, // No file URI — should use inline data
            }]),
        }];

        let input = construct_interactions_input(&history);
        let turns = input.as_array().expect("Expected array");
        let content = turns[0]["content"].as_array().unwrap();

        assert_eq!(content[1]["type"], "image");
        assert_eq!(content[1]["data"], "iVBORw0KGgo=");
        assert_eq!(content[1]["mime_type"], "image/png");
        assert!(content[1].get("uri").is_none());
    }

    #[test]
    fn test_construct_interaction_request_tool_calls() {
        let history = vec![ChatMessage {
            role: "assistant".to_string(),
            content: Some("Let me check.".to_string()),
            reasoning: None,
            tool_calls: Some(vec![ToolCall {
                id: "call_weather_0".to_string(),
                tool_type: "function".to_string(),
                function: FunctionCall {
                    name: "get_weather".to_string(),
                    arguments: "{\"location\": \"London\"}".to_string(),
                },
                thought_signature: None,
            }]),
            tool_call_id: None,
            is_cron: None,
            images: None,
        }];

        let input = construct_interactions_input(&history);
        let turns = input.as_array().expect("Expected array");

        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0]["role"], "model");

        let content = turns[0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 2); // text + function_call

        assert_eq!(content[1]["type"], "function_call");
        assert_eq!(content[1]["id"], "call_weather_0");
        assert_eq!(content[1]["name"], "get_weather");
        assert_eq!(content[1]["arguments"]["location"], "London");
    }

    #[test]
    fn test_construct_interaction_request_tool_response() {
        let history = vec![
            ChatMessage {
                role: "assistant".to_string(),
                content: None,
                reasoning: None,
                tool_calls: Some(vec![ToolCall {
                    id: "call_1".to_string(),
                    tool_type: "function".to_string(),
                    function: FunctionCall {
                        name: "get_weather".to_string(),
                        arguments: "{\"location\": \"Paris\"}".to_string(),
                    },
                    thought_signature: None,
                }]),
                tool_call_id: None,
                is_cron: None,
                images: None,
            },
            ChatMessage {
                role: "tool".to_string(),
                content: Some("Sunny, 25C".to_string()),
                reasoning: None,
                tool_calls: None,
                tool_call_id: Some("call_1".to_string()),
                is_cron: None,
                images: None,
            },
        ];

        let input = construct_interactions_input(&history);
        let turns = input.as_array().expect("Expected array");

        assert_eq!(turns.len(), 2);

        // Tool response turn
        let tool_turn = &turns[1];
        assert_eq!(tool_turn["role"], "user");

        let content = tool_turn["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "function_result");
        assert_eq!(content[0]["name"], "get_weather");
        assert_eq!(content[0]["call_id"], "call_1");
        assert_eq!(content[0]["result"][0]["text"], "Sunny, 25C");
    }

    #[test]
    fn test_construct_interaction_request_tool_response_unknown_call_id() {
        let history = vec![ChatMessage {
            role: "tool".to_string(),
            content: Some("Missing earlier context".to_string()),
            reasoning: None,
            tool_calls: None,
            tool_call_id: Some("orphan_call".to_string()),
            is_cron: None,
            images: None,
        }];

        let input = construct_interactions_input(&history);
        let turns = input.as_array().expect("Expected array");

        assert_eq!(turns.len(), 1);
        let content = turns[0]["content"].as_array().unwrap();

        assert_eq!(content[0]["type"], "function_result");
        assert_eq!(content[0]["name"], "unknown"); // Resolves correctly via default
        assert_eq!(content[0]["call_id"], "orphan_call");
        assert_eq!(content[0]["result"][0]["text"], "Missing earlier context");
    }

    // ========================================================================
    // SSE parsing and streaming tests
    // ========================================================================

    #[test]
    fn test_parse_interaction_sse_text_delta() {
        let sse_line = r#"data: {"event_type":"content.delta","index":0,"delta":{"type":"text","text":"Hello"}}"#;

        let event = parse_interactions_sse_line(sse_line).expect("Failed to parse SSE line");
        assert_eq!(event.event_type, "content.delta");
        assert_eq!(event.index, Some(0));

        if let Some(InteractionDelta::Text { text }) = &event.delta {
            assert_eq!(text, "Hello");
        } else {
            panic!("Expected Text delta, got {:?}", event.delta);
        }
    }

    #[test]
    fn test_parse_interaction_sse_thought_delta() {
        let sse_line = r#"data: {"event_type":"content.delta","index":0,"delta":{"type":"thought","thought":"Thinking..."}}"#;

        let event = parse_interactions_sse_line(sse_line).expect("Failed to parse SSE line");

        if let Some(InteractionDelta::Thought { thought }) = &event.delta {
            assert_eq!(thought.as_deref(), Some("Thinking..."));
        } else {
            panic!("Expected Thought delta, got {:?}", event.delta);
        }
    }

    #[test]
    fn test_parse_interaction_sse_function_call_delta() {
        let sse_line = r#"data: {"event_type":"content.delta","index":1,"delta":{"type":"function_call","id":"fc_123","name":"get_weather","arguments":{"location":"Tokyo"}}}"#;

        let event = parse_interactions_sse_line(sse_line).expect("Failed to parse SSE line");

        if let Some(InteractionDelta::FunctionCallDelta {
            id,
            name,
            arguments,
        }) = &event.delta
        {
            assert_eq!(id, "fc_123");
            assert_eq!(name, "get_weather");
            assert_eq!(arguments["location"], "Tokyo");
        } else {
            panic!("Expected FunctionCallDelta, got {:?}", event.delta);
        }
    }

    #[test]
    fn test_parse_interaction_sse_interaction_complete() {
        let sse_line = r#"data: {"event_type":"interaction.complete","interaction":{"id":"v1_abc","status":"completed","outputs":null}}"#;

        let event = parse_interactions_sse_line(sse_line).expect("Failed to parse SSE line");
        assert_eq!(event.event_type, "interaction.complete");

        let interaction = event.interaction.expect("Expected interaction");
        assert_eq!(interaction.id.as_deref(), Some("v1_abc"));
        assert_eq!(interaction.status.as_deref(), Some("completed"));
    }

    #[test]
    fn test_parse_interaction_sse_ignores_non_data() {
        assert!(parse_interactions_sse_line("event: content.delta").is_none());
        assert!(parse_interactions_sse_line("").is_none());
        assert!(parse_interactions_sse_line("data: [DONE]").is_none());
        assert!(parse_interactions_sse_line("data: ").is_none());
    }

    #[test]
    fn test_parse_interaction_sse_invalid_json() {
        // Logs a warning and returns None instead of panicking or succeeding with partial data
        let sse_line = r#"data: {"error": {"code": 400, "message": "Invalid prompt"}}"#;
        assert!(parse_interactions_sse_line(sse_line).is_none());

        // Malformed JSON (missing closing brace)
        let sse_line_malformed = r#"data: {"event_type":"content.delta""#;
        assert!(parse_interactions_sse_line(sse_line_malformed).is_none());
    }

    #[test]
    fn test_process_interactions_event_text() {
        let event = InteractionStreamEvent {
            event_type: "content.delta".to_string(),
            index: Some(0),
            delta: Some(InteractionDelta::Text {
                text: "Hello world".to_string(),
            }),
            content: None,
            interaction: None,
        };

        let mut full_text = String::new();
        let mut full_reasoning = String::new();
        let mut tool_calls = Vec::new();

        let events = process_interactions_event(
            &event,
            &mut full_text,
            &mut full_reasoning,
            &mut tool_calls,
        );

        assert_eq!(events.len(), 1);
        assert_eq!(full_text, "Hello world");
        assert!(full_reasoning.is_empty());
        assert!(tool_calls.is_empty());
    }

    #[test]
    fn test_process_interactions_event_tool_call() {
        let event = InteractionStreamEvent {
            event_type: "content.delta".to_string(),
            index: Some(1),
            delta: Some(InteractionDelta::FunctionCallDelta {
                id: "fc_abc".to_string(),
                name: "search_wikipedia".to_string(),
                arguments: json!({"query": "Rust programming"}),
            }),
            content: None,
            interaction: None,
        };

        let mut full_text = String::new();
        let mut full_reasoning = String::new();
        let mut tool_calls = Vec::new();

        let events = process_interactions_event(
            &event,
            &mut full_text,
            &mut full_reasoning,
            &mut tool_calls,
        );

        assert_eq!(events.len(), 1);
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].0, "fc_abc");
        assert_eq!(tool_calls[0].1, "search_wikipedia");
        assert_eq!(tool_calls[0].2["query"], "Rust programming");
    }

    #[test]
    fn test_process_interactions_event_accumulates() {
        let mut full_text = String::new();
        let mut full_reasoning = String::new();
        let mut tool_calls = Vec::new();

        // First text delta
        let event1 = InteractionStreamEvent {
            event_type: "content.delta".to_string(),
            index: Some(0),
            delta: Some(InteractionDelta::Text {
                text: "Hello ".to_string(),
            }),
            content: None,
            interaction: None,
        };
        process_interactions_event(
            &event1,
            &mut full_text,
            &mut full_reasoning,
            &mut tool_calls,
        );

        // Second text delta
        let event2 = InteractionStreamEvent {
            event_type: "content.delta".to_string(),
            index: Some(0),
            delta: Some(InteractionDelta::Text {
                text: "world!".to_string(),
            }),
            content: None,
            interaction: None,
        };
        process_interactions_event(
            &event2,
            &mut full_text,
            &mut full_reasoning,
            &mut tool_calls,
        );

        assert_eq!(full_text, "Hello world!");
    }

    #[test]
    fn test_debug_interactions_payload() {
        use crate::agent::{InteractionsRequest, InteractionsTool, InteractionsGenerationConfig};
        let history = vec![
            ChatMessage {
                role: "user".to_string(),
                content: Some("Hi".to_string()),
                reasoning: None,
                tool_calls: None,
                tool_call_id: None,
                is_cron: None,
                images: None,
            },
            ChatMessage {
                role: "assistant".to_string(),
                content: None,
                reasoning: None,
                tool_calls: Some(vec![ToolCall {
                    id: "call_1".to_string(),
                    tool_type: "function".to_string(),
                    function: FunctionCall {
                        name: "web_search".to_string(),
                        arguments: "{\"query\": \"test\"}".to_string(),
                    },
                    thought_signature: None,
                }]),
                tool_call_id: None,
                is_cron: None,
                images: None,
            },
            ChatMessage {
                role: "tool".to_string(),
                content: Some("Search results".to_string()),
                reasoning: None,
                tool_calls: None,
                tool_call_id: Some("call_1".to_string()),
                is_cron: None,
                images: None,
            },
        ];

        let req = InteractionsRequest {
            model: "gemini-3.1-flash-lite-preview".to_string(),
            input: construct_interactions_input(&history),
            system_instruction: Some("test mode".to_string()),
            tools: Some(vec![InteractionsTool::Function {
                name: "web_search".to_string(),
                description: "Search the web".to_string(),
                parameters: serde_json::json!({"type": "object", "properties": {"query": {"type": "string"}}}),
            }]),
            generation_config: Some(InteractionsGenerationConfig {
                thinking_level: Some("low".to_string()),
                thinking_summaries: None,
                temperature: None,
                max_output_tokens: None,
            }),
            stream: true,
            store: Some(false),
        };
    }

    #[tokio::test]
    #[ignore] // Prevent CI from running this without API keys
    async fn test_live_tool_call_invalid_argument() {
        use crate::agent::{InteractionsRequest, InteractionsTool, InteractionsGenerationConfig};
        let keys_res = crate::secrets::get_all_secrets();
        if keys_res.is_err() {
            println!("No keys found, skipping live test");
            return;
        }
        let keys = keys_res.unwrap();
        let api_key = match keys.get("gemini_api_key") {
            Some(key) => key.clone(),
            None => {
                println!("No gemini key found, skipping");
                return;
            }
        };

        let history = vec![
            ChatMessage {
                role: "user".to_string(),
                content: Some("Hi".to_string()),
                reasoning: None,
                tool_calls: None,
                tool_call_id: None,
                is_cron: None,
                images: None,
            },
            ChatMessage {
                role: "assistant".to_string(),
                content: None,
                reasoning: None,
                tool_calls: Some(vec![ToolCall {
                    id: "call_1".to_string(),
                    tool_type: "function".to_string(),
                    function: FunctionCall {
                        name: "web_search".to_string(),
                        arguments: "{\"query\": \"test\"}".to_string(),
                    },
                    thought_signature: None,
                }]),
                tool_call_id: None,
                is_cron: None,
                images: None,
            },
            ChatMessage {
                role: "tool".to_string(),
                content: Some("Search results".to_string()),
                reasoning: None,
                tool_calls: None,
                tool_call_id: Some("call_1".to_string()),
                is_cron: None,
                images: None,
            },
        ];

        let req = InteractionsRequest {
            model: "gemini-3.1-flash-lite-preview".to_string(),
            input: construct_interactions_input(&history),
            system_instruction: Some("test mode".to_string()),
            tools: Some(vec![InteractionsTool::Function {
                name: "web_search".to_string(),
                description: "Search the web".to_string(),
                parameters: serde_json::json!({"type": "object", "properties": {"query": {"type": "string"}}}),
            }]),
            generation_config: Some(InteractionsGenerationConfig {
                thinking_level: Some("high".to_string()),
                thinking_summaries: None,
                temperature: None,
                max_output_tokens: None,
            }),
            stream: true,
            store: Some(false),
        };

        let client = reqwest::Client::new();
        let url = "https://generativelanguage.googleapis.com/v1beta/models/gemini-3.1-flash-lite-preview:interactions";
        let res = client
            .post(url)
            .header("x-goog-api-key", api_key)
            .header("Content-Type", "application/json")
            .json(&req)
            .send()
            .await
            .unwrap();

        let status = res.status();
        let text = res.text().await.unwrap();
        panic!("STATUS: {}\nBODY: {}", status, text);
    }
}

// ----------------------------------------------------------------------------
// DELTA DEBUGGING TEST
// ----------------------------------------------------------------------------
#[cfg(test)]
mod delta_tests {
    use super::*;
    use serde_json::{json, Value};

    #[tokio::test]
    #[ignore]
    async fn test_delta_debug_payload() {
        let keys = crate::secrets::get_all_secrets().expect("needs key");
        let api_key = keys.get("gemini_api_key")
            .cloned()
            .unwrap_or_else(|| panic!("Keys available: {:?}", keys.keys()));
        let raw_json = r#"{
  "model": "gemini-3.1-flash-lite-preview",
  "input": [
    {
      "content": [
        {
          "text": "can you search the web for what I should keep my macbook battery at",
          "type": "text"
        }
      ],
      "role": "user"
    },
    {
      "content": [
        {
          "arguments": {
            "query": "recommended macbook battery percentage for longevity 2026"
          },
          "id": "lymogv85",
          "name": "web_search",
          "type": "function_call"
        }
      ],
      "role": "model"
    },
    {
      "content": [
        {
          "call_id": "lymogv85",
          "name": "web_search",
          "result": [
            {
              "text": "[{\"title\":\"8 Essential Tips\"}]",
              "type": "text"
            }
          ],
          "type": "function_result"
        }
      ],
      "role": "user"
    }
  ],
  "tools": [{"type":"function","name":"web_search","description":"search","parameters":{"type":"object","properties":{}}}]
}"#;

        let mut base_payload: Value = serde_json::from_str(raw_json).unwrap();
        let client = reqwest::Client::new();
        let url = "https://generativelanguage.googleapis.com/v1beta/interactions?key=".to_string() + &api_key;

        // Try base payload
        let res = client.post(&url).json(&base_payload).send().await.unwrap();
        let status = res.status();
        let text = res.text().await.unwrap();
        println!("====== BASE PAYLOAD ======\nSTATUS: {}\nBODY: {}\n", status, text);

        // Variant 1: Remove tools
        let mut variant1 = base_payload.clone();
        variant1.as_object_mut().unwrap().remove("tools");
        let res = client.post(&url).json(&variant1).send().await.unwrap();
        println!("====== NO TOOLS ======\nSTATUS: {}\nBODY: {}\n", res.status(), res.text().await.unwrap());

        // Variant 2: Result as Object
        let mut variant2 = base_payload.clone();
        variant2["input"][2]["content"][0]["result"] = json!({"result":"[{\"title\":\"8 Essential Tips\"}]"});
        let res = client.post(&url).json(&variant2).send().await.unwrap();
        println!("====== RESULT AS OBJECT ======\nSTATUS: {}\nBODY: {}\n", res.status(), res.text().await.unwrap());

        // Variant 3: Result as raw string
        let mut variant3 = base_payload.clone();
        variant3["input"][2]["content"][0]["result"] = json!("[{\"title\":\"8 Essential Tips\"}]");
        let res = client.post(&url).json(&variant3).send().await.unwrap();
        println!("====== RESULT AS STRING ======\nSTATUS: {}\nBODY: {}\n", res.status(), res.text().await.unwrap());

        // Variant 4: Remove generation config
        let mut variant4 = variant1.clone();
        variant4.as_object_mut().unwrap().remove("generation_config");
        let res = client.post(&url).json(&variant4).send().await.unwrap();
        println!("====== NO GENERATION CONFIG ======\nSTATUS: {}\nBODY: {}\n", res.status(), res.text().await.unwrap());

        // Variant 5: Remove system instruction as well
        let mut variant5 = variant4.clone();
        variant5.as_object_mut().unwrap().remove("system_instruction");
        let res = client.post(&url).json(&variant5).send().await.unwrap();
        // We know Variant 6 (Input + function_call, without function_result) throws invalid_argument.
        let mut base_fc = variant5.clone();
        base_fc["input"].as_array_mut().unwrap().pop(); // Pop function_result

        // Test E: Old Gemini Tools Schema
        let mut test_e = base_fc.clone();
        test_e["tools"] = json!([{
            "functionDeclarations": [{
                "name": "web_search",
                "description": "search",
                "parameters": {
                    "type": "OBJECT",
                    "properties": {
                        "query": { "type": "STRING" }
                    }
                }
            }]
        }]);
        let res = client.post(&url).json(&test_e).send().await.unwrap();
        println!("====== FUNC_CALL + CLASSIC TOOLS SCHEMA ======\nSTATUS: {}\nBODY: {}\n", res.status(), res.text().await.unwrap());

        // Test F: Classic Tools Schema + function_result
        let mut test_f = base_payload.clone();
        test_f["tools"] = test_e["tools"].clone();
        let res = client.post(&url).json(&test_f).send().await.unwrap();
        println!("====== FULL HISTORY + CLASSIC TOOLS SCHEMA ======\nSTATUS: {}\nBODY: {}\n", res.status(), res.text().await.unwrap());

        // Test G: Strip `content` wrapping, what if it's just '{"type": "function_call"}' not inside array?
        // Wait, content is strictly an array in Gemini.

        // Test I: Classic Dict functionCall, keeping function_result as interactions API type
        let mut test_i = base_payload.clone();
        test_i["input"][1]["content"][0] = json!({
            "functionCall": {
                "id": "lymogv85",
                "name": "web_search",
                "args": {
                    "query": "battery"
                }
            }
        });
        let res = client.post(&url).json(&test_i).send().await.unwrap();
        println!("====== CLASSIC DICT FUNCTION CALL ======\nSTATUS: {}\nBODY: {}\n", res.status(), res.text().await.unwrap());

        // Test J: Classic Dict functionCall AND functionResponse
        let mut test_j = test_i.clone();
        test_j["input"][2]["content"][0] = json!({
            "functionResponse": {
                "id": "lymogv85",
                "name": "web_search",
                "response": {
                    "result": "[{\"title\":\"8 Essential Tips\"}]"
                }
            }
        });
        let res = client.post(&url).json(&test_j).send().await.unwrap();
        println!("====== CLASSIC DICT BOTH ======\nSTATUS: {}\nBODY: {}\n", res.status(), res.text().await.unwrap());

        panic!("Classic Type debug completed. Check stdout.");
    }
}
