#[cfg(test)]
mod tests {
    use crate::agent::{ChatMessage, ImageAttachment};

    #[test]
    fn test_chat_message_serialization() {
        let msg = ChatMessage {
            role: "user".to_string(),
            content: Some("Hello".to_string()),
            tool_calls: None,
            tool_call_id: None,
            is_cron: None,
            images: None,
            reasoning: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("Hello"));
        assert!(json.contains("user"));
    }

    #[test]
    fn test_chat_message_with_image_serialization() {
        let msg = ChatMessage {
            role: "user".to_string(),
            content: Some("Look at this".to_string()),
            tool_calls: None,
            tool_call_id: None,
            is_cron: None,
            reasoning: None,
            images: Some(vec![ImageAttachment {
                base64: "base64data".to_string(),
                mime_type: "image/png".to_string(),
                file_uri: Some("https://example.com/image.png".to_string()),
            }]),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"images\":[{"));
        assert!(json.contains("base64data"));
        assert!(json.contains("image/png"));
        assert!(json.contains("https://example.com/image.png"));
    }

    #[test]
    fn test_backward_compatible_image_deserialization() {
        use serde_json::json;

        // 1. Modern format: "images" as array
        let modern_json = json!({
            "role": "user",
            "content": "Modern",
            "images": [
                { "base64": "img1", "mime_type": "image/png" },
                { "base64": "img2", "mime_type": "image/jpeg" }
            ]
        })
        .to_string();
        let modern_msg: ChatMessage =
            serde_json::from_str(&modern_json).expect("Failed to parse modern format");
        assert_eq!(modern_msg.images.as_ref().unwrap().len(), 2);
        assert_eq!(modern_msg.images.as_ref().unwrap()[0].base64, "img1");

        // 2. Legacy name: "image" (singular) as single object
        let legacy_singular_json = json!({
            "role": "user",
            "content": "Legacy Singular",
            "image": { "base64": "img-legacy", "mime_type": "image/png" }
        })
        .to_string();
        let legacy_singular_msg: ChatMessage = serde_json::from_str(&legacy_singular_json)
            .expect("Failed to parse legacy singular format");
        assert_eq!(legacy_singular_msg.images.as_ref().unwrap().len(), 1);
        assert_eq!(
            legacy_singular_msg.images.as_ref().unwrap()[0].base64,
            "img-legacy"
        );

        // Verify it serializes back to "images" array
        let reserialized = serde_json::to_string(&legacy_singular_msg).unwrap();
        assert!(reserialized.contains("\"images\":[{"));
        assert!(!reserialized.contains("\"image\":{"));

        // 3. Legacy name: "image" (singular) as array
        let legacy_array_json = json!({
            "role": "user",
            "content": "Legacy Array",
            "image": [
                { "base64": "img-l1", "mime_type": "image/png" },
                { "base64": "img-l2", "mime_type": "image/jpeg" }
            ]
        })
        .to_string();
        let legacy_array_msg: ChatMessage =
            serde_json::from_str(&legacy_array_json).expect("Failed to parse legacy array format");
        assert_eq!(legacy_array_msg.images.as_ref().unwrap().len(), 2);
        assert_eq!(
            legacy_array_msg.images.as_ref().unwrap()[0].base64,
            "img-l1"
        );

        // 4. Mixed: "images" (plural) as single object
        let mixed_json = json!({
            "role": "user",
            "content": "Mixed",
            "images": { "base64": "img-mixed", "mime_type": "image/png" }
        })
        .to_string();
        let mixed_msg: ChatMessage =
            serde_json::from_str(&mixed_json).expect("Failed to parse mixed format");
        assert_eq!(mixed_msg.images.as_ref().unwrap().len(), 1);
        assert_eq!(mixed_msg.images.as_ref().unwrap()[0].base64, "img-mixed");

        // 5. Error handling: Malformed image field
        let malformed_json = json!({
            "role": "user",
            "content": "Malformed",
            "images": "not-an-object-or-array"
        })
        .to_string();
        let err = serde_json::from_str::<ChatMessage>(&malformed_json).unwrap_err();
        assert!(err.to_string().contains("Failed to parse images"));
    }

    // Mocking Tauri AppHandle is difficult in unit tests without extensive setup.
    // Instead, we can test the logic that prepares the API request, if we extract it.
    // For now, let's test the structs and helper functions.

    #[test]
    fn test_gemini_content_structure() {
        // Test that we can construct the Gemini structs correctly
        use crate::agent::{GeminiContent, GeminiFileData, GeminiPart};

        let content = GeminiContent {
            role: Some("user".to_string()),
            parts: vec![
                GeminiPart::Text {
                    text: "Hello".to_string(),
                },
                GeminiPart::FileData {
                    file_data: GeminiFileData {
                        mime_type: "image/png".to_string(),
                        file_uri: "uri".to_string(),
                    },
                },
            ],
        };

        let json = serde_json::to_string(&content).unwrap();
        assert!(json.contains("uri"));
    }

    #[test]
    fn test_construct_gemini_messages() {
        use crate::agent::{construct_gemini_messages, GeminiPart, ProviderMessage};

        let history = vec![
            ProviderMessage {
                role: "user".to_string(),
                content: Some("Hello".to_string()),
                reasoning: None,
                tool_calls: None,
                tool_call_id: None,
                images: None,
            },
            ProviderMessage {
                role: "assistant".to_string(),
                content: Some("Hi there".to_string()),
                reasoning: None,
                tool_calls: None,
                tool_call_id: None,
                images: None,
            },
        ];

        let content = construct_gemini_messages(&history);
        assert_eq!(content.len(), 2);

        if let GeminiPart::Text { text } = &content[0].parts[0] {
            assert_eq!(text, "Hello");
        } else {
            panic!("Expected Text part");
        }

        if let GeminiPart::Text { text } = &content[1].parts[0] {
            assert_eq!(text, "Hi there");
        } else {
            panic!("Expected Text part");
        }
    }

    #[test]
    fn test_parse_gemini_chunk() {
        use crate::agent::{parse_gemini_chunk, AgentEvent, GeminiFunctionCall, GeminiPart};
        use serde_json::json;

        // Test 1: Regular Text
        let mut full_text = String::new();
        let mut full_reasoning = String::new();
        let mut tool_calls = Vec::new();
        let part = GeminiPart::Text {
            text: "Hello".to_string(),
        };
        let events = parse_gemini_chunk(part, &mut full_text, &mut full_reasoning, &mut tool_calls);

        assert_eq!(full_text, "Hello");
        assert_eq!(events.len(), 1);
        if let AgentEvent::ResponseChunk(text) = &events[0] {
            assert_eq!(text, "Hello");
        } else {
            panic!("Expected ResponseChunk");
        }

        // Test 2: Thinking Text
        let mut full_text = String::new();
        let mut full_reasoning = String::new();
        let mut tool_calls = Vec::new();
        let part = GeminiPart::Text {
            text: "**Thinking...**\n\n".to_string(),
        };
        let events = parse_gemini_chunk(part, &mut full_text, &mut full_reasoning, &mut tool_calls);

        assert_eq!(full_reasoning, "**Thinking...**\n\n");
        assert_eq!(events.len(), 1);
        if let AgentEvent::ReasoningChunk(text) = &events[0] {
            assert_eq!(text, "**Thinking...**\n\n");
        } else {
            panic!("Expected ReasoningChunk");
        }

        // Test 3: Explicit Thought Part (thought=true)
        let mut full_text = String::new();
        let mut full_reasoning = String::new();
        let mut tool_calls = Vec::new();
        let part = GeminiPart::Thought {
            thought: true,
            text: "I should check the weather".to_string(),
        };
        let events = parse_gemini_chunk(part, &mut full_text, &mut full_reasoning, &mut tool_calls);

        assert_eq!(full_reasoning, "I should check the weather");
        assert_eq!(events.len(), 1);
        if let AgentEvent::ReasoningChunk(text) = &events[0] {
            assert_eq!(text, "I should check the weather");
        } else {
            panic!("Expected ReasoningChunk");
        }

        // Test 4: Function Call
        let mut full_text = String::new();
        let mut full_reasoning = String::new();
        let mut tool_calls = Vec::new();
        let part = GeminiPart::FunctionCall {
            function_call: GeminiFunctionCall {
                name: "get_weather".to_string(),
                args: json!({"location": "London"}),
            },
            thought_signature: Some("test_signature".to_string()),
        };
        let events = parse_gemini_chunk(part, &mut full_text, &mut full_reasoning, &mut tool_calls);

        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].function_call.name, "get_weather");
        assert_eq!(
            tool_calls[0].thought_signature,
            Some("test_signature".to_string())
        );
        assert_eq!(events.len(), 1);
        if let AgentEvent::ToolCall(fc) = &events[0] {
            assert_eq!(fc.function_call.name, "get_weather");
        } else {
            panic!("Expected ToolCall event");
        }
    }

    // Note: execute_tool is async and requires Agent instance with HTTP client.
    // We can't easily unit test it without mocking the HTTP client or making it public and accepting a client.
    // However, we can test the logic if we extract the match block into a pure function,
    // but it depends on perform_*_lookup which are async and use the client.
    // For now, we rely on integration tests or manual verification for tool execution.
}
