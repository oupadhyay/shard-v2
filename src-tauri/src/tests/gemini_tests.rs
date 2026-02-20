#[cfg(test)]
mod tests {
    use crate::agent::{
        construct_gemini_messages, ChatMessage, FunctionCall, GeminiPart, ImageAttachment, ToolCall,
    };
    use serde_json::json;

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
                images: None,
            },
            ChatMessage {
                role: "assistant".to_string(),
                content: Some("Hi there!".to_string()),
                reasoning: None,
                tool_calls: None,
                tool_call_id: None,
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
                images: None,
            },
            ChatMessage {
                role: "tool".to_string(),
                content: Some("Sunny, 20C".to_string()),
                reasoning: None,
                tool_calls: None,
                tool_call_id: Some("call_1".to_string()),
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
            images: None,
        }];

        let result = construct_gemini_messages(&history);

        // Should be empty because content is None and no images/tool_calls
        assert_eq!(result.len(), 0);
    }
}
