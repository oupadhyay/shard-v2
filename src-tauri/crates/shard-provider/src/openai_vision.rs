//! OpenAI-compatible vision request shaping and HTTP transport.
//!
//! Callers choose providers and models, compose prompts, and own fallback and
//! retry policy. This module accepts those decisions explicitly, builds the
//! multimodal wire request, sends it, and extracts the response text.

use std::time::Duration;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct OpenAiVisionTransportConfig {
    pub endpoint_url: String,
    pub auth_token: String,
    pub timeout: Duration,
}

#[derive(Debug, Clone)]
pub struct OpenAiVisionRequest {
    pub model: String,
    pub prompt: String,
    pub image_base64: String,
    pub mime_type: String,
    pub max_completion_tokens: Option<u32>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
}

#[derive(Serialize, Debug)]
struct VisionRequestBody {
    model: String,
    messages: Vec<VisionMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_completion_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

#[derive(Serialize, Debug)]
struct VisionMessage {
    role: String,
    content: Vec<VisionContent>,
}

#[derive(Serialize, Debug)]
#[serde(tag = "type")]
enum VisionContent {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image_url")]
    ImageUrl { image_url: ImageUrlPayload },
}

#[derive(Serialize, Debug)]
struct ImageUrlPayload {
    url: String,
}

#[derive(Deserialize, Debug)]
struct OpenAiResponse {
    choices: Option<Vec<OpenAiChoice>>,
}

#[derive(Deserialize, Debug)]
struct OpenAiChoice {
    message: OpenAiMessage,
}

#[derive(Deserialize, Debug)]
struct OpenAiMessage {
    content: Option<String>,
}

impl OpenAiVisionRequest {
    fn to_wire_body(&self) -> VisionRequestBody {
        VisionRequestBody {
            model: self.model.clone(),
            messages: vec![VisionMessage {
                role: "user".to_string(),
                content: vec![
                    VisionContent::Text {
                        text: self.prompt.clone(),
                    },
                    VisionContent::ImageUrl {
                        image_url: ImageUrlPayload {
                            url: format!("data:{};base64,{}", self.mime_type, self.image_base64),
                        },
                    },
                ],
            }],
            max_completion_tokens: self.max_completion_tokens,
            max_tokens: self.max_tokens,
            temperature: self.temperature,
        }
    }
}

pub async fn analyze_image(
    client: &reqwest::Client,
    config: &OpenAiVisionTransportConfig,
    request: &OpenAiVisionRequest,
) -> Result<String, String> {
    let response = client
        .post(&config.endpoint_url)
        .header("Authorization", format!("Bearer {}", config.auth_token))
        .header("Content-Type", "application/json")
        .timeout(config.timeout)
        .json(&request.to_wire_body())
        .send()
        .await
        .map_err(|error| format!("Vision API network error: {}", error))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        let body_preview = body.chars().take(200).collect::<String>();
        return Err(format!("Vision API error {}: {}", status, body_preview));
    }

    let body: OpenAiResponse = response
        .json()
        .await
        .map_err(|error| format!("Failed to parse vision response: {}", error))?;

    body.choices
        .and_then(|choices| choices.into_iter().next())
        .and_then(|choice| choice.message.content)
        .ok_or_else(|| "No content in vision response".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{body_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn request() -> OpenAiVisionRequest {
        OpenAiVisionRequest {
            model: "vision-model".to_string(),
            prompt: "What is shown?".to_string(),
            image_base64: "abc123".to_string(),
            mime_type: "image/png".to_string(),
            max_completion_tokens: Some(2048),
            max_tokens: None,
            temperature: Some(0.7),
        }
    }

    #[test]
    fn vision_request_wire_shape_is_stable() {
        let serialized = serde_json::to_string(&request().to_wire_body()).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&serialized).unwrap(),
            json!({
                "model": "vision-model",
                "messages": [{
                    "role": "user",
                    "content": [
                        {
                            "type": "text",
                            "text": "What is shown?"
                        },
                        {
                            "type": "image_url",
                            "image_url": {
                                "url": "data:image/png;base64,abc123"
                            }
                        }
                    ]
                }],
                "max_completion_tokens": 2048,
                "temperature": 0.7
            })
        );
    }

    #[tokio::test]
    async fn sends_configured_request_and_extracts_content() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(header("authorization", "Bearer test-token"))
            .and(header("content-type", "application/json"))
            .and(body_json(json!({
                "model": "vision-model",
                "messages": [{
                    "role": "user",
                    "content": [
                        {"type": "text", "text": "What is shown?"},
                        {
                            "type": "image_url",
                            "image_url": {"url": "data:image/png;base64,abc123"}
                        }
                    ]
                }],
                "max_completion_tokens": 2048,
                "temperature": 0.7
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "choices": [{"message": {"content": "A test image"}}]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let content = analyze_image(
            &reqwest::Client::new(),
            &OpenAiVisionTransportConfig {
                endpoint_url: format!("{}/v1/chat/completions", server.uri()),
                auth_token: "test-token".to_string(),
                timeout: Duration::from_secs(5),
            },
            &request(),
        )
        .await
        .expect("vision request should succeed");

        assert_eq!(content, "A test image");
    }

    #[tokio::test]
    async fn reports_unsuccessful_status_with_bounded_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(429).set_body_string("rate limited"))
            .mount(&server)
            .await;

        let error = analyze_image(
            &reqwest::Client::new(),
            &OpenAiVisionTransportConfig {
                endpoint_url: format!("{}/v1/chat/completions", server.uri()),
                auth_token: "test-token".to_string(),
                timeout: Duration::from_secs(5),
            },
            &request(),
        )
        .await
        .expect_err("HTTP failure should be reported");

        assert_eq!(
            error,
            "Vision API error 429 Too Many Requests: rate limited"
        );
    }

    #[tokio::test]
    async fn reports_missing_response_content() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"choices": []})))
            .mount(&server)
            .await;

        let error = analyze_image(
            &reqwest::Client::new(),
            &OpenAiVisionTransportConfig {
                endpoint_url: format!("{}/v1/chat/completions", server.uri()),
                auth_token: "test-token".to_string(),
                timeout: Duration::from_secs(5),
            },
            &request(),
        )
        .await
        .expect_err("empty choices should be rejected");

        assert_eq!(error, "No content in vision response");
    }
}
