//! Vision fallback orchestration for image understanding.
//!
//! Ownership split: this workflow stays in the Shard host because it decides
//! when non-vision chat models need image-to-text preprocessing, which fallback
//! models to try, and how to prompt them with user/app context. The underlying
//! OpenAI-compatible request/transport pieces can later share provider helpers.

use reqwest::Client;
use serde::{Deserialize, Serialize};

/// Groq Vision model (Llama 4 Scout with vision capabilities)
const GROQ_VISION_MODEL: &str = "meta-llama/llama-4-scout-17b-16e-instruct";

/// OpenRouter free vision models in priority order (fallback if Gemma 4 26B MoE fails)
const OPENROUTER_VISION_MODELS: &[&str] = &["nvidia/nemotron-nano-12b-v2-vl:free"];

#[derive(Debug, Clone)]
pub struct VisionLlmEndpoints {
    pub openrouter_chat_url: String,
    pub groq_chat_url: String,
}

#[derive(Debug, Clone)]
pub struct VisionLlmConfig {
    pub openrouter_auth_token: Option<String>,
    pub groq_auth_token: Option<String>,
    pub endpoints: VisionLlmEndpoints,
}

#[derive(Serialize, Debug)]
struct OpenAIVisionRequest {
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
    url: String, // data:image/png;base64,... format
}

#[derive(Deserialize, Debug)]
struct OpenAIResponse {
    choices: Option<Vec<OpenAIChoice>>,
}

#[derive(Deserialize, Debug)]
struct OpenAIChoice {
    message: OpenAIMessage,
}

#[derive(Deserialize, Debug)]
struct OpenAIMessage {
    content: Option<String>,
}

/// Process an image with the user's actual question for contextual understanding.
/// This is the single entry point for all vision processing - screenshots, chat images,
/// and screen context analysis all use this function.
///
/// Uses Gemma 4 31B as the primary vision model, with fallbacks to other vision models.
pub async fn process_image_with_context(
    http_client: &Client,
    image_base64: &str,
    mime_type: &str,
    user_question: &str,
    config: &VisionLlmConfig,
) -> Result<String, String> {
    // Primary model: Gemma 4 26B-A4B MoE — role-separated from chat's 31B dense
    const CONTEXT_VISION_MODEL: &str = "google/gemma-4-26b-a4b-it:free";

    // Build a prompt that includes the user's question for contextual understanding
    let contextual_prompt = format!(
        r#"You are a helpful vision assistant. The user has attached an image and asked a question about it.

USER'S QUESTION: {}

Please analyze the image carefully and provide a helpful response that directly addresses the user's question. Focus on the visual elements that are relevant to their query. Be detailed but concise."#,
        user_question
    );

    let data_uri = format!("data:{};base64,{}", mime_type, image_base64);

    // Try OpenRouter with Gemma 4 31B
    if let Some(openrouter_key) = &config.openrouter_auth_token {
        log::info!(
            "[VisionLLM] Processing image with context using {}",
            CONTEXT_VISION_MODEL
        );

        let request = OpenAIVisionRequest {
            model: CONTEXT_VISION_MODEL.to_string(),
            messages: vec![VisionMessage {
                role: "user".to_string(),
                content: vec![
                    VisionContent::Text {
                        text: contextual_prompt.clone(),
                    },
                    VisionContent::ImageUrl {
                        image_url: ImageUrlPayload {
                            url: data_uri.clone(),
                        },
                    },
                ],
            }],
            max_completion_tokens: Some(2048), // More tokens for detailed contextual response
            max_tokens: None,
            temperature: Some(0.7),
        };

        let response = http_client
            .post(&config.endpoints.openrouter_chat_url)
            .header("Authorization", format!("Bearer {}", openrouter_key))
            .header("Content-Type", "application/json")
            .timeout(std::time::Duration::from_secs(45)) // Longer timeout for contextual response
            .json(&request)
            .send()
            .await;

        if let Ok(resp) = response {
            if resp.status().is_success() {
                if let Ok(body) = resp.json::<OpenAIResponse>().await {
                    if let Some(content) = body
                        .choices
                        .and_then(|c| c.into_iter().next())
                        .and_then(|choice| choice.message.content)
                    {
                        log::info!(
                            "[VisionLLM] Contextual vision success with {} ({} chars)",
                            CONTEXT_VISION_MODEL,
                            content.len()
                        );
                        return Ok(content);
                    }
                }
            } else {
                let status = resp.status();
                let error_text = resp.text().await.unwrap_or_default();
                log::warn!(
                    "[VisionLLM] {} failed with status {}: {}",
                    CONTEXT_VISION_MODEL,
                    status,
                    &error_text[..error_text.len().min(200)]
                );
            }
        }

        // Fallback to other vision models if Gemma 4 fails
        for model in OPENROUTER_VISION_MODELS {
            let request = OpenAIVisionRequest {
                model: model.to_string(),
                messages: vec![VisionMessage {
                    role: "user".to_string(),
                    content: vec![
                        VisionContent::Text {
                            text: contextual_prompt.clone(),
                        },
                        VisionContent::ImageUrl {
                            image_url: ImageUrlPayload {
                                url: data_uri.clone(),
                            },
                        },
                    ],
                }],
                max_completion_tokens: Some(2048),
                max_tokens: None,
                temperature: Some(0.7),
            };

            log::info!("[VisionLLM] Trying fallback vision model: {}", model);

            let response = http_client
                .post(&config.endpoints.openrouter_chat_url)
                .header("Authorization", format!("Bearer {}", openrouter_key))
                .header("Content-Type", "application/json")
                .timeout(std::time::Duration::from_secs(45))
                .json(&request)
                .send()
                .await;

            if let Ok(resp) = response {
                if resp.status().is_success() {
                    if let Ok(body) = resp.json::<OpenAIResponse>().await {
                        if let Some(content) = body
                            .choices
                            .and_then(|c| c.into_iter().next())
                            .and_then(|choice| choice.message.content)
                        {
                            log::info!("[VisionLLM] Fallback success with {}", model);
                            return Ok(content);
                        }
                    }
                }
            }
        }
    }

    // Final fallback to Groq if OpenRouter unavailable
    if let Some(groq_key) = &config.groq_auth_token {
        log::info!("[VisionLLM] Trying Groq for contextual vision...");

        let request = OpenAIVisionRequest {
            model: GROQ_VISION_MODEL.to_string(),
            messages: vec![VisionMessage {
                role: "user".to_string(),
                content: vec![
                    VisionContent::Text {
                        text: contextual_prompt,
                    },
                    VisionContent::ImageUrl {
                        image_url: ImageUrlPayload { url: data_uri },
                    },
                ],
            }],
            max_completion_tokens: Some(2048),
            max_tokens: None,
            temperature: Some(0.7),
        };

        let response = http_client
            .post(&config.endpoints.groq_chat_url)
            .header("Authorization", format!("Bearer {}", groq_key))
            .header("Content-Type", "application/json")
            .timeout(std::time::Duration::from_secs(45))
            .json(&request)
            .send()
            .await
            .map_err(|e| format!("Network error: {}", e))?;

        if response.status().is_success() {
            let body: OpenAIResponse = response
                .json()
                .await
                .map_err(|e| format!("Failed to parse response: {}", e))?;

            return body
                .choices
                .and_then(|c| c.into_iter().next())
                .and_then(|choice| choice.message.content)
                .ok_or_else(|| "No content in response".to_string());
        }
    }

    Err(
        "No Vision LLM API key configured or all attempts failed for contextual processing"
            .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vision_content_serialization() {
        let content = VisionContent::Text {
            text: "Hello".to_string(),
        };
        let json = serde_json::to_string(&content).unwrap();
        assert!(json.contains("\"type\":\"text\""));
        assert!(json.contains("\"text\":\"Hello\""));

        let image_content = VisionContent::ImageUrl {
            image_url: ImageUrlPayload {
                url: "data:image/png;base64,abc123".to_string(),
            },
        };
        let json = serde_json::to_string(&image_content).unwrap();
        assert!(json.contains("\"type\":\"image_url\""));
        assert!(json.contains("\"url\":\"data:image/png;base64,abc123\""));
    }
}
