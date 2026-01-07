/// Vision LLM module - Use Groq or OpenRouter vision models for image understanding
/// This replaces Tesseract OCR with API-based vision model calls for better
/// multilingual support and the ability to understand images without text.
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::config::AppConfig;

/// Default prompt for OCR-like image description
const VISION_PROMPT: &str = "Identify the subject of this image specifically (e.g., 'Steam logo', 'Python code', 'Error message'). Extract ALL visible text exactly as shown. Describe key visual details (colors, shapes, layout) concisely but precisely as if you were describing it to a blind person.";

/// Groq Vision model (Llama 4 Scout with vision capabilities)
const GROQ_VISION_MODEL: &str = "meta-llama/llama-4-scout-17b-16e-instruct";

/// OpenRouter free vision models in priority order
const OPENROUTER_VISION_MODELS: &[&str] = &[
    "google/gemma-3-27b-it:free",
    "nvidia/nemotron-nano-12b-v2-vl:free",
];

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
    error: Option<OpenAIError>,
}

#[derive(Deserialize, Debug)]
struct OpenAIChoice {
    message: OpenAIMessage,
}

#[derive(Deserialize, Debug)]
struct OpenAIMessage {
    content: Option<String>,
}

#[derive(Deserialize, Debug)]
struct OpenAIError {
    message: String,
}

/// Describe an image using a Vision LLM.
/// Tries OpenRouter first if API key is available, falls back to Groq.
pub async fn describe_image(
    http_client: &Client,
    image_base64: &str,
    mime_type: &str,
    config: &AppConfig,
) -> Result<String, String> {
    // Try OpenRouter first (priority 1)
    if let Some(openrouter_key) = &config.openrouter_api_key {
        log::info!("[VisionLLM] Attempting OpenRouter Vision...");

        for model in OPENROUTER_VISION_MODELS {
            match call_vision_api(
                http_client,
                "https://openrouter.ai/api/v1/chat/completions",
                openrouter_key,
                model,
                image_base64,
                mime_type,
            )
            .await
            {
                Ok(result) => {
                    log::info!(
                        "[VisionLLM] OpenRouter Vision success with model: {}",
                        model
                    );
                    return Ok(result);
                }
                Err(e) => {
                    log::warn!("[VisionLLM] OpenRouter model {} failed: {}", model, e);
                }
            }
        }
    }

    // Fallback to Groq (priority 2)
    if let Some(groq_key) = &config.groq_api_key {
        log::info!("[VisionLLM] Attempting Groq Vision...");
        match call_vision_api(
            http_client,
            "https://api.groq.com/openai/v1/chat/completions",
            groq_key,
            GROQ_VISION_MODEL,
            image_base64,
            mime_type,
        )
        .await
        {
            Ok(result) => {
                log::info!("[VisionLLM] Groq Vision success");
                return Ok(result);
            }
            Err(e) => {
                log::warn!(
                    "[VisionLLM] Groq Vision failed: {}",
                    e
                );
            }
        }
    }

    // No API keys available or all failed
    Err("No OpenRouter or Groq API key configured (or all attempts failed) for Vision LLM".to_string())
}

/// Call an OpenAI-compatible vision API endpoint
async fn call_vision_api(
    http_client: &Client,
    url: &str,
    api_key: &str,
    model: &str,
    image_base64: &str,
    mime_type: &str,
) -> Result<String, String> {
    let data_uri = format!("data:{};base64,{}", mime_type, image_base64);

    let request = OpenAIVisionRequest {
        model: model.to_string(),
        messages: vec![VisionMessage {
            role: "user".to_string(),
            content: vec![
                VisionContent::Text {
                    text: VISION_PROMPT.to_string(),
                },
                VisionContent::ImageUrl {
                    image_url: ImageUrlPayload { url: data_uri },
                },
            ],
        }],
        max_completion_tokens: Some(1024),
        max_tokens: None,
        temperature: Some(1.0),
    };

    let response = http_client
        .post(url)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&request)
        .send()
        .await
        .map_err(|e| format!("Network error: {}", e))?;

    if !response.status().is_success() {
        let status = response.status();
        let error_text = response.text().await.unwrap_or_default();
        return Err(format!("API error {}: {}", status, error_text));
    }

    let body: OpenAIResponse = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse response: {}", e))?;

    if let Some(error) = body.error {
        return Err(format!("API returned error: {}", error.message));
    }

    body.choices
        .and_then(|c| c.into_iter().next())
        .and_then(|choice| choice.message.content)
        .ok_or_else(|| "No content in response".to_string())
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
