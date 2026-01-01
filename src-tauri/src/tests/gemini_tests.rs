#[cfg(test)]
mod tests {
    use crate::agent::GeminiPart;
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

        let part: GeminiPart = serde_json::from_value(json_data).expect("Failed to deserialize FunctionCall");

        if let GeminiPart::FunctionCall { function_call, thought_signature: _ } = part {
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

        let part: GeminiPart = serde_json::from_value(json_data).expect("Failed to deserialize Text");

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

        let part: GeminiPart = serde_json::from_value(json_data).expect("Failed to deserialize FileData");

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

        let part: GeminiPart = serde_json::from_value(json_data).expect("Failed to deserialize FunctionResponse");

        if let GeminiPart::FunctionResponse { function_response } = part {
            assert_eq!(function_response.name, "get_weather");
            assert_eq!(function_response.response["result"], "Sunny, 25C");
        } else {
            panic!("Expected FunctionResponse variant");
        }
    }
}
