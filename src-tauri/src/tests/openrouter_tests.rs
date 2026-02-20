#[cfg(test)]
mod tests {
    use crate::agent::{
        has_images, supports_tools, to_multimodal_messages, ChatMessage, FunctionCall,
        ImageAttachment, ToolCall,
    };

    #[test]
    fn test_to_multimodal_messages_text_only() {
        let messages = vec![ChatMessage {
            role: "user".to_string(),
            content: Some("Hello".to_string()),
            reasoning: None,
            tool_calls: None,
            tool_call_id: None,
            images: None,
        }];

        let result = to_multimodal_messages(&messages);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["role"], "user");
        assert_eq!(result[0]["content"], "Hello");
    }

    #[test]
    fn test_to_multimodal_messages_single_image() {
        let messages = vec![
            ChatMessage {
                role: "user".to_string(),
                content: Some("What is this?".to_string()),
                reasoning: None,
                tool_calls: None,
                tool_call_id: None,
                images: Some(vec![ImageAttachment {
                    base64: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==".to_string(),
                    mime_type: "image/png".to_string(),
                    file_uri: None,
                }]),
            },
        ];

        let result = to_multimodal_messages(&messages);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["role"], "user");

        let content = result[0]["content"]
            .as_array()
            .expect("Content should be an array");
        assert_eq!(content.len(), 2);

        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "What is this?");

        assert_eq!(content[1]["type"], "image_url");
        assert_eq!(content[1]["image_url"]["url"], "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==");
    }

    #[test]
    fn test_to_multimodal_messages_multiple_images() {
        let messages = vec![ChatMessage {
            role: "user".to_string(),
            content: Some("Compare these".to_string()),
            reasoning: None,
            tool_calls: None,
            tool_call_id: None,
            images: Some(vec![
                ImageAttachment {
                    base64: "img1".to_string(),
                    mime_type: "image/jpeg".to_string(),
                    file_uri: None,
                },
                ImageAttachment {
                    base64: "img2".to_string(),
                    mime_type: "image/png".to_string(),
                    file_uri: None,
                },
            ]),
        }];

        let result = to_multimodal_messages(&messages);
        assert_eq!(result.len(), 1);

        let content = result[0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 3); // 1 text + 2 images

        assert_eq!(content[0]["text"], "Compare these");
        assert_eq!(
            content[1]["image_url"]["url"],
            "data:image/jpeg;base64,img1"
        );
        assert_eq!(content[2]["image_url"]["url"], "data:image/png;base64,img2");
    }

    #[test]
    fn test_to_multimodal_messages_images_only() {
        let messages = vec![ChatMessage {
            role: "user".to_string(),
            content: None,
            reasoning: None,
            tool_calls: None,
            tool_call_id: None,
            images: Some(vec![ImageAttachment {
                base64: "img_only".to_string(),
                mime_type: "image/webp".to_string(),
                file_uri: None,
            }]),
        }];

        let result = to_multimodal_messages(&messages);
        assert_eq!(result.len(), 1);

        let content = result[0]["content"]
            .as_array()
            .expect("Content should be an array even without text");
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "image_url");
        assert_eq!(
            content[0]["image_url"]["url"],
            "data:image/webp;base64,img_only"
        );
    }

    #[test]
    fn test_to_multimodal_messages_tool_calls() {
        let messages = vec![ChatMessage {
            role: "assistant".to_string(),
            content: Some("Checking weather...".to_string()),
            reasoning: None,
            tool_calls: Some(vec![ToolCall {
                id: "call_123".to_string(),
                tool_type: "function".to_string(),
                function: FunctionCall {
                    name: "get_weather".to_string(),
                    arguments: "{\"location\":\"London\"}".to_string(),
                },
                thought_signature: None,
            }]),
            tool_call_id: None,
            images: None,
        }];

        let result = to_multimodal_messages(&messages);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["role"], "assistant");
        assert_eq!(result[0]["content"], "Checking weather...");

        let tool_calls = result[0]["tool_calls"]
            .as_array()
            .expect("Should have tool_calls");
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0]["id"], "call_123");
        assert_eq!(tool_calls[0]["function"]["name"], "get_weather");
    }

    #[test]
    fn test_to_multimodal_messages_tool_result() {
        let messages = vec![ChatMessage {
            role: "tool".to_string(),
            content: Some("Sunny, 20C".to_string()),
            reasoning: None,
            tool_calls: None,
            tool_call_id: Some("call_123".to_string()),
            images: None,
        }];

        let result = to_multimodal_messages(&messages);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["role"], "tool");
        assert_eq!(result[0]["content"], "Sunny, 20C");
        assert_eq!(result[0]["tool_call_id"], "call_123");
    }

    #[test]
    fn test_to_multimodal_messages_empty_content_with_images() {
        let messages = vec![ChatMessage {
            role: "user".to_string(),
            content: Some("".to_string()),
            reasoning: None,
            tool_calls: None,
            tool_call_id: None,
            images: Some(vec![ImageAttachment {
                base64: "data".to_string(),
                mime_type: "image/png".to_string(),
                file_uri: None,
            }]),
        }];

        let result = to_multimodal_messages(&messages);
        assert_eq!(result.len(), 1);
        let content = result[0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 1); // Should only have image_url, no empty text part
        assert_eq!(content[0]["type"], "image_url");
    }

    #[test]
    fn test_to_multimodal_messages_empty_image_list() {
        let messages = vec![ChatMessage {
            role: "user".to_string(),
            content: Some("Hello".to_string()),
            reasoning: None,
            tool_calls: None,
            tool_call_id: None,
            images: Some(vec![]),
        }];

        let result = to_multimodal_messages(&messages);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0]["content"], "Hello"); // Should fallback to regular text message
    }

    #[test]
    fn test_has_images() {
        let messages_no_images = vec![ChatMessage {
            role: "user".to_string(),
            content: Some("Hello".to_string()),
            reasoning: None,
            tool_calls: None,
            tool_call_id: None,
            images: None,
        }];
        assert!(!has_images(&messages_no_images));

        let messages_with_images = vec![ChatMessage {
            role: "user".to_string(),
            content: Some("Look at this".to_string()),
            reasoning: None,
            tool_calls: None,
            tool_call_id: None,
            images: Some(vec![ImageAttachment {
                base64: "data".to_string(),
                mime_type: "image/png".to_string(),
                file_uri: None,
            }]),
        }];
        assert!(has_images(&messages_with_images));

        let messages_with_empty_images = vec![ChatMessage {
            role: "user".to_string(),
            content: Some("Look at this".to_string()),
            reasoning: None,
            tool_calls: None,
            tool_call_id: None,
            images: Some(vec![]),
        }];
        assert!(!has_images(&messages_with_empty_images));
    }

    #[test]
    fn test_supports_tools() {
        assert!(supports_tools("gpt-4o"));
        assert!(!supports_tools("olmo-3.1-32b-think"));
    }
}
