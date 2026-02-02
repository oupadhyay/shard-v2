/// Vision LLM module - Use Groq or OpenRouter vision models for image understanding
/// This replaces Tesseract OCR with API-based vision model calls for better
/// multilingual support and the ability to understand images without text.
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::config::AppConfig;

/// Default prompt for OCR-like image description
const VISION_PROMPT: &str = r#"Act as an exhaustive OCR engine with a focus on RELEVANCE. Your goal is to extract text, but you MUST prioritize the PRIMARY content.

CRITICAL PRIORITIZATION:
1. PRIMARY FOCUS: Identify the largest, most central, or active window. Focus 90% of your effort here.
2. FOREGROUND ONLY: Prioritize text in the active window (e.g., the models list, the code editor, the main article).
3. IGNORE NOISE: Treat background tabs (e.g., YouTube tabs in the background), system clocks, and menu bars as secondary noise. Do NOT let them distract from the main content.
4. STRUCTURE: Group text by "Primary Window" and "Secondary/Background".

INSTRUCTIONS:
1. Extract ALL text from the Primary Window in detail.
2. Extract only minimal, identifying text from Background elements.
3. Be literal: Provide the exact text as it appears.
4. Contextualize: Provide a 2-sentence summary that identifies the SINGLE most important thing the user is doing.

OUTPUT FORMAT:
[Primary Window: (Name/Title)]
- Text item 1
- Text item 2
...

[Background/Secondary]
- (Minimal identifying text only)

Summary: [2-sentence summary of the PRIMARY activity]"#;

/// Groq Vision model (Llama 4 Scout with vision capabilities)
const GROQ_VISION_MODEL: &str = "meta-llama/llama-4-scout-17b-16e-instruct";

/// OpenRouter free vision models in priority order
const OPENROUTER_VISION_MODELS: &[&str] = &[
    "allenai/molmo-2-8b:free",
    "qwen/qwen-2.5-vl-7b-instruct:free",
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
    _agent: &crate::agent::Agent,
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
                log::warn!("[VisionLLM] Groq Vision failed: {}", e);
            }
        }
    }

    // No API keys available or all failed
    Err(
        "No OpenRouter or Groq API key configured (or all attempts failed) for Vision LLM"
            .to_string(),
    )
}

/// Process an image with the user's actual question for contextual understanding.
/// This is used when the selected chat model doesn't support vision - we use a
/// vision-capable model (Gemma 3 27B) to understand the image in context of the query.
///
/// Unlike `describe_image()` which uses a fixed OCR prompt, this function passes
/// the user's actual question to enable contextual visual understanding.
pub async fn process_image_with_context(
    http_client: &Client,
    image_base64: &str,
    mime_type: &str,
    user_question: &str,
    config: &AppConfig,
) -> Result<String, String> {
    // Primary model: Gemma 3 27B - strong multimodal capabilities
    const CONTEXT_VISION_MODEL: &str = "google/gemma-3-27b-it:free";

    // Build a prompt that includes the user's question for contextual understanding
    let contextual_prompt = format!(
        r#"You are a helpful vision assistant. The user has attached an image and asked a question about it.

USER'S QUESTION: {}

Please analyze the image carefully and provide a helpful response that directly addresses the user's question. Focus on the visual elements that are relevant to their query. Be detailed but concise."#,
        user_question
    );

    let data_uri = format!("data:{};base64,{}", mime_type, image_base64);

    // Try OpenRouter with Gemma 3 27B
    if let Some(openrouter_key) = &config.openrouter_api_key {
        log::info!("[VisionLLM] Processing image with context using {}", CONTEXT_VISION_MODEL);

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
            .post("https://openrouter.ai/api/v1/chat/completions")
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

        // Fallback to other vision models if Gemma 3 fails
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
                .post("https://openrouter.ai/api/v1/chat/completions")
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
    if let Some(groq_key) = &config.groq_api_key {
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
            .post("https://api.groq.com/openai/v1/chat/completions")
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

    Err("No Vision LLM API key configured or all attempts failed for contextual processing".to_string())
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

    log::info!("[VisionLLM] Sending request to {} with model {}...", url, model);

    let response = http_client
        .post(url)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .timeout(std::time::Duration::from_secs(30)) // 30s timeout for vision
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

/// Analyze an image with a custom prompt (for screen context, etc.)
pub async fn analyze_with_prompt(
    agent: &crate::agent::Agent,
    http_client: &Client,
    image_base64: Option<&str>,
    mime_type: Option<&str>,
    prompt: &str,
    config: &AppConfig,
) -> Result<String, String> {
    // If no image is provided, use the primary chat model for text-only analysis
    if image_base64.is_none() {
        log::info!("[VisionLLM] Using text-only analysis via Agent...");
        return agent.call_chat_completion(prompt, config).await;
    }

    // Vision-based analysis (original logic)
    let image_base64 = image_base64.unwrap();
    let mime_type = mime_type.unwrap_or("image/png");
    let data_uri = format!("data:{};base64,{}", mime_type, image_base64);

    // Try OpenRouter first
    if let Some(openrouter_key) = &config.openrouter_api_key {
        for model in OPENROUTER_VISION_MODELS {
            let request = OpenAIVisionRequest {
                model: model.to_string(),
                messages: vec![VisionMessage {
                    role: "user".to_string(),
                    content: vec![
                        VisionContent::Text {
                            text: prompt.to_string(),
                        },
                        VisionContent::ImageUrl {
                            image_url: ImageUrlPayload {
                                url: data_uri.clone(),
                            },
                        },
                    ],
                }],
                max_completion_tokens: Some(4096),
                max_tokens: None,
                temperature: Some(0.7),
            };

            log::info!("[VisionLLM] Trying OpenRouter vision model: {}", model);

            let response = http_client
                .post("https://openrouter.ai/api/v1/chat/completions")
                .header("Authorization", format!("Bearer {}", openrouter_key))
                .header("Content-Type", "application/json")
                .json(&request)
                .send()
                .await;

            if let Ok(resp) = response {
                let status = resp.status();
                if status.is_success() {
                    if let Ok(body) = resp.json::<OpenAIResponse>().await {
                        if let Some(content) = body
                            .choices
                            .and_then(|c| c.into_iter().next())
                            .and_then(|choice| choice.message.content)
                        {
                            log::info!("[VisionLLM] Success with model: {}", model);
                            log::debug!(
                                "[VisionLLM] Response: {}",
                                &content[..content.len().min(200)]
                            );
                            return Ok(content);
                        }
                    }
                } else {
                    log::warn!("[VisionLLM] Model {} returned status: {}", model, status);
                }
            }
        }
    }

    // Fallback to Groq
    if let Some(groq_key) = &config.groq_api_key {
        let request = OpenAIVisionRequest {
            model: GROQ_VISION_MODEL.to_string(),
            messages: vec![VisionMessage {
                role: "user".to_string(),
                content: vec![
                    VisionContent::Text {
                        text: prompt.to_string(),
                    },
                    VisionContent::ImageUrl {
                        image_url: ImageUrlPayload { url: data_uri },
                    },
                ],
            }],
            max_completion_tokens: Some(1024),
            max_tokens: None,
            temperature: Some(0.7),
        };

        let response = http_client
            .post("https://api.groq.com/openai/v1/chat/completions")
            .header("Authorization", format!("Bearer {}", groq_key))
            .header("Content-Type", "application/json")
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

    Err("No Vision LLM API key configured or all attempts failed".to_string())
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
