/**
 * Centralized model definitions for the Shard application.
 *
 * This module provides a single source of truth for all available LLM models,
 * their providers, and capabilities. The frontend consumes this via the
 * `get_available_models` Tauri command.
 *
 * Architecture: Role separation ensures no single model serves chat, vision,
 * AND background simultaneously. Gemma 4 31B (dense) handles chat; Gemma 4
 * 26B-A4B (MoE) handles vision + background.
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
    /// Unique identifier used in API calls (e.g., "google/gemma-4-26b-a4b-it:free")
    pub id: String,
    /// Human-readable name for UI display (e.g., "Gemma 4 26B MoE")
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
        // Gemini — primary tier
        ModelInfo {
            id: "gemma-4-31b-it".to_string(),
            display_name: "Gemma 4 31B".to_string(),
            provider: Provider::Gemini,
            category: Category::Chat,
            supports_tools: true,
            supports_vision: true, // Native multimodal (text + image), 256K context
        },
        ModelInfo {
            id: "gemini-3-flash-preview".to_string(),
            display_name: "3 Flash Preview".to_string(),
            provider: Provider::Gemini,
            category: Category::Chat,
            supports_tools: true,
            supports_vision: true, // 1M context, frontier-class reasoning
        },
        ModelInfo {
            id: "gemini-3.1-flash-lite-preview".to_string(),
            display_name: "3.1 Flash Lite".to_string(),
            provider: Provider::Gemini,
            category: Category::Chat,
            supports_tools: true,
            supports_vision: true, // 1M context, ultra-low TTFT, configurable thinking levels
        },
        // OpenRouter / Groq — fallback tier
        ModelInfo {
            id: "openai/gpt-oss-120b:free".to_string(),
            display_name: "GPT-OSS 120B".to_string(),
            provider: Provider::OpenRouter,
            category: Category::Chat,
            supports_tools: true,
            supports_vision: false, // Text-only, 128K context
        },
        ModelInfo {
            id: "gpt-oss-120b (Groq)".to_string(),
            display_name: "GPT-OSS 120B (Groq)".to_string(),
            provider: Provider::Groq,
            category: Category::Chat,
            supports_tools: true,
            supports_vision: false, // Text-only, 128K context, Groq LPU speed
        },
    ]
}

// ============================================================================
// VISION MODELS - Used by Vision LLM module for image description
// ============================================================================

/// Vision-capable models for the Vision LLM module.
/// Uses Gemma 4 26B-A4B (MoE) to avoid overlapping with chat's 31B dense.
pub fn get_vision_models() -> Vec<ModelInfo> {
    vec![
        ModelInfo {
            id: "google/gemma-4-26b-a4b-it:free".to_string(),
            display_name: "Gemma 4 26B MoE".to_string(),
            provider: Provider::OpenRouter,
            category: Category::Vision,
            supports_tools: true,
            supports_vision: true, // MoE (3.8B active), native multimodal, 256K context
        },
        ModelInfo {
            id: "nvidia/nemotron-nano-12b-v2-vl:free".to_string(),
            display_name: "Nemotron Nano 12B V2 VL".to_string(),
            provider: Provider::OpenRouter,
            category: Category::Vision,
            supports_tools: true,
            supports_vision: true, // OCR/document specialist
        },
    ]
}

// ============================================================================
// BACKGROUND MODELS - Background job dropdown
// ============================================================================

/// Models available for background jobs.
/// Default is Gemma 4 26B-A4B (MoE) via Gemini API for 1500/day rate limit
/// and role separation from the 31B dense chat model.
pub fn get_background_models() -> Vec<ModelInfo> {
    vec![
        // Gemini — default (MoE for role separation from chat's dense 31B)
        ModelInfo {
            id: "gemma-4-26b-a4b-it".to_string(),
            display_name: "Gemma 4 26B MoE (Gemini)".to_string(),
            provider: Provider::Gemini,
            category: Category::Background,
            supports_tools: true,
            supports_vision: true, // MoE, 1500 req/day free tier
        },
        // Groq — fast alternative
        ModelInfo {
            id: "gpt-oss-120b (Groq)".to_string(),
            display_name: "GPT-OSS 120B (Groq)".to_string(),
            provider: Provider::Groq,
            category: Category::Background,
            supports_tools: true,
            supports_vision: false, // Text-only, Groq LPU speed
        },
        // Gemini — dense 31B for higher quality background when needed
        ModelInfo {
            id: "gemma-4-31b-it".to_string(),
            display_name: "Gemma 4 31B (Gemini)".to_string(),
            provider: Provider::Gemini,
            category: Category::Background,
            supports_tools: true,
            supports_vision: true, // Dense 31B, higher quality
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
        assert_eq!(models.len(), 5, "Should have exactly 5 chat models");
    }

    #[test]
    fn test_vision_models() {
        let models = get_vision_models();
        let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(models.len(), 2, "Should have exactly 2 vision models");
        assert!(ids.contains(&"google/gemma-4-26b-a4b-it:free"), "Primary vision: Gemma 4 26B MoE");
        assert!(ids.contains(&"nvidia/nemotron-nano-12b-v2-vl:free"), "Fallback vision: Nemotron Nano");
    }

    #[test]
    fn test_background_models() {
        let models = get_background_models();
        assert_eq!(models.len(), 3, "Should have exactly 3 background models");
        let default = models.first().unwrap();
        assert_eq!(default.id, "gemma-4-26b-a4b-it", "Default background should be 26B MoE");
        assert_eq!(default.provider, Provider::Gemini);
    }

    #[test]
    fn test_role_separation() {
        // Chat default and vision/background default should be different models
        let chat_default = get_chat_models().first().unwrap().id.clone();
        let vision_primary = get_vision_models().first().unwrap().id.clone();
        let bg_default = get_background_models().first().unwrap().id.clone();

        assert_ne!(chat_default, vision_primary, "Chat and vision should use different models");
        assert_ne!(chat_default, bg_default, "Chat and background defaults should differ");
    }

    #[test]
    fn test_gemma4_31b_is_gemini_model() {
        assert!(is_gemini_model("gemma-4-31b-it"));
        assert!(is_gemini_model("gemma-4-26b-a4b-it"));
        // OpenRouter variant should NOT be classified as Gemini
        assert!(!is_gemini_model("google/gemma-4-26b-a4b-it:free"));
    }

    #[test]
    fn test_no_cerebras_provider() {
        // Verify Cerebras has been fully removed from all model lists
        let all_models: Vec<ModelInfo> = get_chat_models()
            .into_iter()
            .chain(get_vision_models())
            .chain(get_background_models())
            .collect();

        for model in &all_models {
            assert!(
                !format!("{:?}", model.provider).contains("Cerebras"),
                "Model {} should not use Cerebras provider", model.id
            );
            assert!(
                !model.id.contains("Cerebras"),
                "Model ID {} should not reference Cerebras", model.id
            );
        }
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
