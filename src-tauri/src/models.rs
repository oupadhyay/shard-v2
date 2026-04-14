/**
 * Centralized model definitions for the Shard application.
 *
 * This module provides a single source of truth for all available LLM models,
 * their providers, and capabilities. The frontend consumes this via the
 * `get_available_models` Tauri command.
 */
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::LazyLock;

/// Provider identifies which API endpoint handles this model
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Gemini,
    OpenRouter,
    Groq,
    Cerebras,
}

/// Category determines which dropdown(s) a model appears in
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    /// Main chat models (settings Chat Model dropdown)
    Chat,
    /// Vision-capable models (used by Vision LLM module for image description)
    Vision,
    /// Background job models (settings Background Job Model dropdown)
    Background,
}

/// Complete model information for frontend consumption
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    /// Unique identifier used in API calls (e.g., "google/gemma-3-27b-it:free")
    pub id: String,
    /// Human-readable name for UI display (e.g., "Gemma 3-27B")
    pub display_name: String,
    /// Which provider handles this model
    pub provider: Provider,
    /// Which dropdown(s) this model appears in
    pub category: Category,
    /// Whether this model supports function/tool calling
    pub supports_tools: bool,
    /// Whether this model can directly process images (vision-capable)
    pub supports_vision: bool,
}

// ============================================================================
// CHAT MODELS - Main chat dropdown
// ============================================================================

/// All available chat models
pub fn get_chat_models() -> Vec<ModelInfo> {
    vec![
        // Gemini
        ModelInfo {
            id: "gemma-4-31b-it".to_string(),
            display_name: "Gemma 4 31B".to_string(),
            provider: Provider::Gemini,
            category: Category::Chat,
            supports_tools: true,
            supports_vision: true, // Native multimodal (text + image)
        },
        ModelInfo {
            id: "gemini-3.1-flash-lite-preview".to_string(),
            display_name: "3.1 Flash Lite".to_string(),
            provider: Provider::Gemini,
            category: Category::Chat,
            supports_tools: true,
            supports_vision: true, // Native vision via Files API
        },
        ModelInfo {
            id: "gemini-2.5-flash".to_string(),
            display_name: "2.5 Flash".to_string(),
            provider: Provider::Gemini,
            category: Category::Chat,
            supports_tools: true,
            supports_vision: true, // Native vision via Files API
        },
        ModelInfo {
            id: "gemini-3-flash-preview".to_string(),
            display_name: "3 Flash Preview".to_string(),
            provider: Provider::Gemini,
            category: Category::Chat,
            supports_tools: true,
            supports_vision: true, // Native vision via Files API
        },
        // OpenRouter
        ModelInfo {
            id: "openrouter/free".to_string(),
            display_name: "Auto (free)".to_string(),
            provider: Provider::OpenRouter,
            category: Category::Chat,
            supports_tools: true,
            supports_vision: true, // Router filters for vision support if images are sent
        },
        ModelInfo {
            id: "openai/gpt-oss-120b:free".to_string(),
            display_name: "GPT-OSS 120B".to_string(),
            provider: Provider::OpenRouter,
            category: Category::Chat,
            supports_tools: true,
            supports_vision: false, // Text-only model
        },
        ModelInfo {
            id: "stepfun/step-3.5-flash:free".to_string(),
            display_name: "Stepfun 3.5 Flash".to_string(),
            provider: Provider::OpenRouter,
            category: Category::Chat,
            supports_tools: true,
            supports_vision: false, // Text-only model
        },
        ModelInfo {
            id: "meta-llama/llama-3.3-70b-instruct:free".to_string(),
            display_name: "LLaMA 3.3 70B".to_string(),
            provider: Provider::OpenRouter,
            category: Category::Chat,
            supports_tools: true,
            supports_vision: false, // Text-only model
        },
        // Other Providers
        ModelInfo {
            id: "gpt-oss-120b (Cerebras)".to_string(),
            display_name: "GPT-OSS 120B (Cerebras)".to_string(),
            provider: Provider::Cerebras,
            category: Category::Chat,
            supports_tools: true,
            supports_vision: false, // Text-only model
        },
        ModelInfo {
            id: "gpt-oss-120b (Groq)".to_string(),
            display_name: "GPT-OSS 120B (Groq)".to_string(),
            provider: Provider::Groq,
            category: Category::Chat,
            supports_tools: true,
            supports_vision: false, // Text-only model
        },
    ]
}

// ============================================================================
// VISION MODELS - Used by Vision LLM module for image description
// ============================================================================

/// Vision-capable models for the Vision LLM module
pub fn get_vision_models() -> Vec<ModelInfo> {
    vec![
        ModelInfo {
            id: "google/gemma-4-31b-it:free".to_string(),
            display_name: "Gemma 4 31B".to_string(),
            provider: Provider::OpenRouter,
            category: Category::Vision,
            supports_tools: true,
            supports_vision: true, // Native multimodal (text + image)
        },
        ModelInfo {
            id: "allenai/molmo-2-8b:free".to_string(),
            display_name: "Molmo 2-8B".to_string(),
            provider: Provider::OpenRouter,
            category: Category::Vision,
            supports_tools: false,
            supports_vision: true, // Vision model
        },
        ModelInfo {
            id: "nvidia/nemotron-nano-12b-v2-vl:free".to_string(),
            display_name: "Nemotron Nano 12B V2 VL".to_string(),
            provider: Provider::OpenRouter,
            category: Category::Vision,
            supports_tools: true,
            supports_vision: true, // Vision model
        },
        ModelInfo {
            id: "qwen/qwen-2.5-vl-7b-instruct:free".to_string(),
            display_name: "Qwen 2.5 VL 7B Instruct".to_string(),
            provider: Provider::OpenRouter,
            category: Category::Vision,
            supports_tools: false,
            supports_vision: true, // Vision model
        },
    ]
}

// ============================================================================
// BACKGROUND MODELS - Background job dropdown
// ============================================================================

/// Models available for background jobs
pub fn get_background_models() -> Vec<ModelInfo> {
    vec![
        // Gemini (native API — better rate limits: 1500/day)
        ModelInfo {
            id: "gemma-4-31b-it".to_string(),
            display_name: "Gemma 4 31B (Gemini)".to_string(),
            provider: Provider::Gemini,
            category: Category::Background,
            supports_tools: true,
            supports_vision: true, // Native multimodal
        },
        // Groq
        ModelInfo {
            id: "gpt-oss-20b (Groq)".to_string(),
            display_name: "GPT-OSS 20B (Groq)".to_string(),
            provider: Provider::Groq,
            category: Category::Background,
            supports_tools: true,
            supports_vision: false, // Text-only
        },
        ModelInfo {
            id: "gpt-oss-120b (Groq)".to_string(),
            display_name: "GPT-OSS 120B (Groq)".to_string(),
            provider: Provider::Groq,
            category: Category::Background,
            supports_tools: true,
            supports_vision: false, // Text-only
        },
        // Cerebras
        ModelInfo {
            id: "gpt-oss-120b (Cerebras)".to_string(),
            display_name: "GPT-OSS 120B (Cerebras)".to_string(),
            provider: Provider::Cerebras,
            category: Category::Background,
            supports_tools: true,
            supports_vision: false, // Text-only
        },
        ModelInfo {
            id: "llama-3.3-70b (Cerebras)".to_string(),
            display_name: "LLaMA 3.3 70B (Cerebras)".to_string(),
            provider: Provider::Cerebras,
            category: Category::Background,
            supports_tools: true,
            supports_vision: false, // Text-only
        },
        // OpenRouter
        ModelInfo {
            id: "google/gemma-4-31b-it:free (OpenRouter)".to_string(),
            display_name: "Gemma 4 31B (OpenRouter)".to_string(),
            provider: Provider::OpenRouter,
            category: Category::Background,
            supports_tools: true,
            supports_vision: true, // Native multimodal
        },
        ModelInfo {
            id: "google/gemma-3-27b-it:free (OpenRouter)".to_string(),
            display_name: "Gemma 3-27B (OpenRouter)".to_string(),
            provider: Provider::OpenRouter,
            category: Category::Background,
            supports_tools: true,
            supports_vision: true, // Multimodal
        },
        ModelInfo {
            id: "openai/gpt-oss-20b:free (OpenRouter)".to_string(),
            display_name: "GPT-OSS 20B (OpenRouter)".to_string(),
            provider: Provider::OpenRouter,
            category: Category::Background,
            supports_tools: true,
            supports_vision: false, // Text-only
        },
    ]
}

/// Static registry mapping model IDs to (provider, supports_vision).
/// Built once on first access from all model lists.
static MODEL_REGISTRY: LazyLock<HashMap<String, (Provider, bool)>> = LazyLock::new(|| {
    let mut map = HashMap::new();
    for model in get_chat_models()
        .into_iter()
        .chain(get_vision_models())
        .chain(get_background_models())
    {
        map.insert(model.id, (model.provider, model.supports_vision));
    }
    map
});

/// Check if a model ID corresponds to a Gemini provider model.
/// Uses the model registry first; falls back to heuristic.
pub fn is_gemini_model(model_id: &str) -> bool {
    if let Some((provider, _)) = MODEL_REGISTRY.get(model_id) {
        return *provider == Provider::Gemini;
    }

    // Fallback heuristic for unknown model IDs:
    // Gemini-native models have no slash, no provider suffix, and start with "gemini" or "gemma"
    (model_id.starts_with("gemini") || model_id.starts_with("gemma"))
        && !model_id.contains('/')
        && !model_id.contains("(Cerebras)")
        && !model_id.contains("(Groq)")
}

/// Check if a model ID supports native vision (direct image processing).
/// Returns false for unknown models (they will use the Vision LLM fallback).
pub fn model_supports_vision(model_id: &str) -> bool {
    MODEL_REGISTRY
        .get(model_id)
        .map(|(_, vision)| *vision)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chat_models_not_empty() {
        let models = get_chat_models();
        assert!(!models.is_empty());
    }

    #[test]
    fn test_vision_models_contain_new_additions() {
        let models = get_vision_models();
        let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
        assert!(ids.contains(&"google/gemma-4-31b-it:free"));
        assert!(ids.contains(&"allenai/molmo-2-8b:free"));
        assert!(ids.contains(&"nvidia/nemotron-nano-12b-v2-vl:free"));
        assert!(ids.contains(&"qwen/qwen-2.5-vl-7b-instruct:free"));
    }

    #[test]
    fn test_gemma4_chat_model() {
        let models = get_chat_models();
        let gemma4 = models.iter().find(|m| m.id == "gemma-4-31b-it");
        assert!(gemma4.is_some(), "Gemma 4 31B should be in chat models");
        let gemma4 = gemma4.unwrap();
        assert_eq!(gemma4.provider, Provider::Gemini);
        assert!(gemma4.supports_tools);
        assert!(gemma4.supports_vision);
    }

    #[test]
    fn test_gemma4_is_gemini_model() {
        assert!(is_gemini_model("gemma-4-31b-it"));
        // OpenRouter variant should NOT be classified as Gemini
        assert!(!is_gemini_model("google/gemma-4-31b-it:free"));
    }

    #[test]
    fn test_gemma4_background_model() {
        let models = get_background_models();
        let gemini_entry = models.iter().find(|m| m.id == "gemma-4-31b-it");
        assert!(gemini_entry.is_some(), "Gemma 4 31B (Gemini) should be in background models");
        assert_eq!(gemini_entry.unwrap().provider, Provider::Gemini);
        let or_entry = models.iter().find(|m| m.id == "google/gemma-4-31b-it:free (OpenRouter)");
        assert!(or_entry.is_some(), "Gemma 4 31B (OpenRouter) should also be in background models");
    }

    #[test]
    fn test_model_serialization() {
        let model = ModelInfo {
            id: "test-model".to_string(),
            display_name: "Test Model".to_string(),
            provider: Provider::Gemini,
            category: Category::Chat,
            supports_tools: true,
            supports_vision: false,
        };

        let json = serde_json::to_string(&model).unwrap();
        assert!(json.contains("test-model"));
        assert!(json.contains("gemini"));

        let deserialized: ModelInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, "test-model");
    }
}
