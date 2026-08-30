//! Vision fallback orchestration for image understanding.
//!
//! Ownership split: this workflow stays in the Shard host because it decides
//! when non-vision chat models need image-to-text preprocessing, which fallback
//! models to try, and how to prompt them with user/app context. The underlying
//! OpenAI-compatible request shaping and transport live in `shard-provider`.

use std::time::Duration;

use reqwest::Client;
use shard_provider::{analyze_image, OpenAiVisionRequest, OpenAiVisionTransportConfig};

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

const VISION_REQUEST_TIMEOUT: Duration = Duration::from_secs(45);

fn vision_request(
    model: &str,
    prompt: &str,
    image_base64: &str,
    mime_type: &str,
) -> OpenAiVisionRequest {
    OpenAiVisionRequest {
        model: model.to_string(),
        prompt: prompt.to_string(),
        image_base64: image_base64.to_string(),
        mime_type: mime_type.to_string(),
        max_completion_tokens: Some(2048),
        max_tokens: None,
        temperature: Some(0.7),
    }
}

fn transport_config(endpoint_url: &str, auth_token: &str) -> OpenAiVisionTransportConfig {
    OpenAiVisionTransportConfig {
        endpoint_url: endpoint_url.to_string(),
        auth_token: auth_token.to_string(),
        timeout: VISION_REQUEST_TIMEOUT,
    }
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

    // Try OpenRouter with Gemma 4 31B
    if let Some(openrouter_key) = &config.openrouter_auth_token {
        log::info!(
            "[VisionLLM] Processing image with context using {}",
            CONTEXT_VISION_MODEL
        );

        let transport = transport_config(&config.endpoints.openrouter_chat_url, openrouter_key);
        let request = vision_request(
            CONTEXT_VISION_MODEL,
            &contextual_prompt,
            image_base64,
            mime_type,
        );

        match analyze_image(http_client, &transport, &request).await {
            Ok(content) => {
                log::info!(
                    "[VisionLLM] Contextual vision success with {} ({} chars)",
                    CONTEXT_VISION_MODEL,
                    content.len()
                );
                return Ok(content);
            }
            Err(error) => {
                log::warn!("[VisionLLM] {} failed: {}", CONTEXT_VISION_MODEL, error);
            }
        }

        // Fallback to other vision models if Gemma 4 fails
        for model in OPENROUTER_VISION_MODELS {
            let request = vision_request(model, &contextual_prompt, image_base64, mime_type);

            log::info!("[VisionLLM] Trying fallback vision model: {}", model);

            if let Ok(content) = analyze_image(http_client, &transport, &request).await {
                log::info!("[VisionLLM] Fallback success with {}", model);
                return Ok(content);
            }
        }
    }

    // Final fallback to Groq if OpenRouter unavailable
    if let Some(groq_key) = &config.groq_auth_token {
        log::info!("[VisionLLM] Trying Groq for contextual vision...");

        let transport = transport_config(&config.endpoints.groq_chat_url, groq_key);
        let request = vision_request(
            GROQ_VISION_MODEL,
            &contextual_prompt,
            image_base64,
            mime_type,
        );

        return analyze_image(http_client, &transport, &request).await;
    }

    Err(
        "No Vision LLM API key configured or all attempts failed for contextual processing"
            .to_string(),
    )
}
