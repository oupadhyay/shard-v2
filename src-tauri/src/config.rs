use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use tauri::{AppHandle, Manager, Runtime};

const CONFIG_FILENAME: &str = "config.toml";

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AppConfig {
    // API keys - stored in OS keyring at runtime, but serializable to frontend
    // Note: These are manually cleared before saving to TOML (see save_config_internal)
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub gemini_api_key: Option<String>,
    #[serde(default)]
    pub openrouter_api_key: Option<String>,
    #[serde(default)]
    pub cerebras_api_key: Option<String>,
    #[serde(default)]
    pub brave_api_key: Option<String>,
    #[serde(default)]
    pub groq_api_key: Option<String>,

    // Non-sensitive settings - stored in config.toml
    pub selected_model: Option<String>,
    pub api_base_url: Option<String>, // e.g., https://generativelanguage.googleapis.com/v1beta/openai/
    pub enable_web_search: Option<bool>,
    pub enable_tools: Option<bool>,
    pub system_prompt: Option<String>, // Custom system prompt, if None will use MCP default
    pub incognito_mode: Option<bool>,
    pub research_mode: Option<bool>,
    pub background_model: Option<String>,
    // Auto-retry configuration
    pub max_auto_retries: Option<u32>, // Default: 2
    pub retry_on_empty: Option<bool>,  // Retry empty responses after reasoning
    pub retry_on_katex: Option<bool>,  // Retry on frontend KaTeX parse errors
    // Screen context capture
    pub enable_screen_context: Option<bool>, // Default: false
    // Compaction configuration
    pub enable_compaction: Option<bool>,        // Default: true
    pub compaction_threshold: Option<f32>,      // Default: 0.5 (50%)
    pub compaction_preserve_turns: Option<u32>, // Default: 5
    // Fallback model for quota errors
    #[serde(default)]
    pub fallback_model: Option<String>, // Default: openai/gpt-oss-120b:free
    // Heartbeat engine configuration
    #[serde(default)]
    pub heartbeat_global_cooldown_secs: Option<u64>, // Default: 60
}

#[derive(Debug, Clone)]
pub struct ModelProviderConfig {
    pub base_url: String,
    pub model_id: String,
    pub provider_name: String,
    pub reasoning_effort: Option<String>,
}

impl ModelProviderConfig {
    /// Get the full URL for chat completions
    pub fn full_url(&self) -> String {
        format!("{}chat/completions", self.base_url)
    }
}

impl AppConfig {
    /// Get provider mapping and API key for a model name.
    /// Returns (ProviderConfig, ApiKey)
    pub fn get_model_provider_config(
        &self,
        model: &str,
        context: &str,
    ) -> Result<(ModelProviderConfig, String), String> {
        let (provider_name, base_url, model_id, reasoning_effort) = if model.contains("(Cerebras)")
        {
            let base_model = model.replace(" (Cerebras)", "").trim().to_string();
            let model_id = if base_model.contains("120b") {
                "gpt-oss-120b".to_string()
            } else if base_model.contains("70b") {
                "llama-3.3-70b".to_string()
            } else {
                base_model
            };
            (
                "Cerebras",
                "https://api.cerebras.ai/v1/",
                model_id,
                Some("high".to_string()),
            )
        } else if model.contains("(Groq)") {
            let base_model = model.replace(" (Groq)", "").trim().to_string();
            let model_id = if base_model.contains("120b") {
                "openai/gpt-oss-120b".to_string()
            } else if base_model.contains("20b") {
                "openai/gpt-oss-20b".to_string()
            } else {
                format!("openai/{}", base_model)
            };
            (
                "Groq",
                "https://api.groq.com/openai/v1/",
                model_id,
                Some("high".to_string()),
            )
        } else {
            let model_id = model
                .split(" (OpenRouter)")
                .next()
                .unwrap_or(model)
                .trim()
                .to_string();
            let model_id = if model_id.is_empty() {
                "google/gemma-4-31b-it:free".to_string()
            } else {
                model_id
            };
            (
                "OpenRouter",
                "https://openrouter.ai/api/v1/",
                model_id,
                None,
            )
        };

        let key = match provider_name {
            "Cerebras" => &self.cerebras_api_key,
            "Groq" => &self.groq_api_key,
            _ => &self.openrouter_api_key,
        };

        match key {
            Some(k) => Ok((
                ModelProviderConfig {
                    base_url: base_url.to_string(),
                    model_id,
                    provider_name: provider_name.to_string(),
                    reasoning_effort,
                },
                k.clone(),
            )),
            None => Err(format!(
                "No {} API key configured for {}",
                provider_name, context
            )),
        }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            api_key: None,
            gemini_api_key: None,
            openrouter_api_key: None,
            cerebras_api_key: None,
            brave_api_key: None,
            selected_model: None,
            api_base_url: None,
            enable_web_search: None,
            enable_tools: Some(true),
            system_prompt: None,
            incognito_mode: None,
            research_mode: Some(false),
            groq_api_key: None,
            background_model: Some("gpt-oss-120b (Groq)".to_string()),
            // Auto-retry defaults
            max_auto_retries: Some(2),
            retry_on_empty: Some(true),
            retry_on_katex: Some(true),
            // Screen context capture (off by default for privacy)
            enable_screen_context: Some(false),
            // Compaction defaults
            enable_compaction: Some(true),
            compaction_threshold: Some(0.5),
            compaction_preserve_turns: Some(5),
            // Fallback model for quota errors
            fallback_model: Some("openai/gpt-oss-120b:free".to_string()),
            heartbeat_global_cooldown_secs: Some(60),
        }
    }
}

pub fn get_config_path<R: Runtime>(app_handle: &AppHandle<R>) -> Result<PathBuf, String> {
    let resolver = app_handle.path();
    match resolver.app_config_dir() {
        Ok(dir) => Ok(dir.join(CONFIG_FILENAME)),
        Err(e) => Err(format!("Failed to get app config directory: {}", e)),
    }
}

pub fn load_config<R: Runtime>(app_handle: &AppHandle<R>) -> Result<AppConfig, String> {
    use crate::secrets::{self, ApiKeyType};

    let config_path = get_config_path(app_handle)?;
    let mut loaded = if !config_path.exists() {
        AppConfig::default()
    } else {
        let content = fs::read_to_string(&config_path)
            .map_err(|e| format!("Failed to read config file: {}", e))?;
        toml::from_str::<AppConfig>(&content)
            .map_err(|e| format!("Failed to parse config file: {}", e))?
    };

    // Load all keys from keyring with a single cache-backed call.
    // secrets::get_all_secrets() reads the keychain at most once per process;
    // all subsequent calls serve from the in-memory cache.
    let all_keys = secrets::get_all_secrets().unwrap_or_default();
    loaded.api_key = all_keys.get(ApiKeyType::OpenAI.key_name()).cloned();
    loaded.gemini_api_key = all_keys.get(ApiKeyType::Gemini.key_name()).cloned();
    loaded.openrouter_api_key = all_keys.get(ApiKeyType::OpenRouter.key_name()).cloned();
    loaded.cerebras_api_key = all_keys.get(ApiKeyType::Cerebras.key_name()).cloned();
    loaded.brave_api_key = all_keys.get(ApiKeyType::Brave.key_name()).cloned();
    loaded.groq_api_key = all_keys.get(ApiKeyType::Groq.key_name()).cloned();

    // Merge with defaults for optional fields
    let defaults = AppConfig::default();
    if loaded.enable_compaction.is_none() {
        loaded.enable_compaction = defaults.enable_compaction;
    }
    if loaded.compaction_threshold.is_none() {
        loaded.compaction_threshold = defaults.compaction_threshold;
    }
    if loaded.compaction_preserve_turns.is_none() {
        loaded.compaction_preserve_turns = defaults.compaction_preserve_turns;
    }
    if loaded.heartbeat_global_cooldown_secs.is_none() {
        loaded.heartbeat_global_cooldown_secs = defaults.heartbeat_global_cooldown_secs;
    }

    Ok(loaded)
}

/// Internal save that just writes TOML (no keyring interaction)
/// Clears API keys before serialization to prevent saving sensitive data to disk
fn save_config_internal(config_path: &PathBuf, config: &AppConfig) -> Result<(), String> {
    if let Some(parent_dir) = config_path.parent() {
        if !parent_dir.exists() {
            fs::create_dir_all(parent_dir)
                .map_err(|e| format!("Failed to create config directory: {}", e))?;
        }
    }

    // Clone and clear API keys before serializing to TOML
    let mut config_for_toml = config.clone();
    config_for_toml.api_key = None;
    config_for_toml.gemini_api_key = None;
    config_for_toml.openrouter_api_key = None;
    config_for_toml.cerebras_api_key = None;
    config_for_toml.brave_api_key = None;
    config_for_toml.groq_api_key = None;

    let toml_string = toml::to_string_pretty(&config_for_toml)
        .map_err(|e| format!("Failed to serialize config: {}", e))?;
    fs::write(config_path, toml_string).map_err(|e| format!("Failed to write config file: {}", e))
}

pub fn save_config<R: Runtime>(
    app_handle: &AppHandle<R>,
    config: &AppConfig,
) -> Result<(), String> {
    use crate::secrets::{self, ApiKeyType};

    // Save API keys to keyring in a single batch operation
    use std::collections::HashMap;
    let mut updates = HashMap::new();

    updates.insert(ApiKeyType::OpenAI, config.api_key.clone());
    updates.insert(ApiKeyType::Gemini, config.gemini_api_key.clone());
    updates.insert(ApiKeyType::OpenRouter, config.openrouter_api_key.clone());
    updates.insert(ApiKeyType::Cerebras, config.cerebras_api_key.clone());
    updates.insert(ApiKeyType::Brave, config.brave_api_key.clone());
    updates.insert(ApiKeyType::Groq, config.groq_api_key.clone());

    secrets::store_secrets_batch(updates)?;

    // Save non-sensitive config to TOML (API keys cleared before serialization)
    let config_path = get_config_path(app_handle)?;
    save_config_internal(&config_path, config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cerebras_provider_config() {
        let config = AppConfig {
            cerebras_api_key: Some("test-key".to_string()),
            ..Default::default()
        };
        let (pc, key) = config
            .get_model_provider_config("gpt-oss-120b (Cerebras)", "test")
            .unwrap();
        assert_eq!(pc.provider_name, "Cerebras");
        assert_eq!(pc.base_url, "https://api.cerebras.ai/v1/");
        assert_eq!(pc.model_id, "gpt-oss-120b");
        assert_eq!(key, "test-key");
        assert_eq!(pc.reasoning_effort, Some("high".to_string()));
    }

    #[test]
    fn test_groq_provider_config() {
        let config = AppConfig {
            groq_api_key: Some("groq-key".to_string()),
            ..Default::default()
        };
        let (pc, key) = config
            .get_model_provider_config("gpt-oss-120b (Groq)", "test")
            .unwrap();
        assert_eq!(pc.provider_name, "Groq");
        assert_eq!(pc.base_url, "https://api.groq.com/openai/v1/");
        assert_eq!(pc.model_id, "openai/gpt-oss-120b");
        assert_eq!(key, "groq-key");
    }

    #[test]
    fn test_openrouter_provider_config() {
        let config = AppConfig {
            openrouter_api_key: Some("or-key".to_string()),
            ..Default::default()
        };
        let (pc, key) = config
            .get_model_provider_config("google/gemma-3-27b-it:free", "test")
            .unwrap();
        assert_eq!(pc.provider_name, "OpenRouter");
        assert_eq!(pc.base_url, "https://openrouter.ai/api/v1/");
        assert_eq!(pc.model_id, "google/gemma-3-27b-it:free");
        assert_eq!(key, "or-key");
        assert_eq!(pc.reasoning_effort, None);
    }

    #[test]
    fn test_cerebras_70b_model() {
        let config = AppConfig {
            cerebras_api_key: Some("key".to_string()),
            ..Default::default()
        };
        let (pc, _) = config
            .get_model_provider_config("llama-3.3-70b (Cerebras)", "test")
            .unwrap();
        assert_eq!(pc.model_id, "llama-3.3-70b");
        assert_eq!(pc.provider_name, "Cerebras");
    }

    #[test]
    fn test_cerebras_generic_model() {
        let config = AppConfig {
            cerebras_api_key: Some("key".to_string()),
            ..Default::default()
        };
        let (pc, _) = config
            .get_model_provider_config("some-new-model (Cerebras)", "test")
            .unwrap();
        assert_eq!(pc.model_id, "some-new-model");
    }

    #[test]
    fn test_groq_20b_model() {
        let config = AppConfig {
            groq_api_key: Some("key".to_string()),
            ..Default::default()
        };
        let (pc, _) = config
            .get_model_provider_config("gpt-oss-20b (Groq)", "test")
            .unwrap();
        assert_eq!(pc.model_id, "openai/gpt-oss-20b");
    }

    #[test]
    fn test_groq_generic_model() {
        let config = AppConfig {
            groq_api_key: Some("key".to_string()),
            ..Default::default()
        };
        let (pc, _) = config
            .get_model_provider_config("llama-4-scout (Groq)", "test")
            .unwrap();
        assert_eq!(pc.model_id, "openai/llama-4-scout");
    }

    #[test]
    fn test_openrouter_with_suffix() {
        let config = AppConfig {
            openrouter_api_key: Some("key".to_string()),
            ..Default::default()
        };
        let (pc, _) = config
            .get_model_provider_config("google/gemma-3-27b-it:free (OpenRouter)", "test")
            .unwrap();
        assert_eq!(pc.model_id, "google/gemma-3-27b-it:free");
    }

    #[test]
    fn test_full_url() {
        let pc = ModelProviderConfig {
            base_url: "https://api.cerebras.ai/v1/".to_string(),
            model_id: "gpt-oss-120b".to_string(),
            provider_name: "Cerebras".to_string(),
            reasoning_effort: None,
        };
        assert_eq!(pc.full_url(), "https://api.cerebras.ai/v1/chat/completions");
    }

    #[test]
    fn test_missing_api_key_error() {
        let config = AppConfig::default();
        let err = config
            .get_model_provider_config("gpt-oss-120b (Cerebras)", "main chat")
            .unwrap_err();
        assert!(err.contains("Cerebras"));
        assert!(err.contains("main chat"));
    }

    #[test]
    fn test_missing_groq_key_error() {
        let config = AppConfig::default();
        let err = config
            .get_model_provider_config("gpt-oss-120b (Groq)", "background jobs")
            .unwrap_err();
        assert!(err.contains("Groq"));
        assert!(err.contains("background jobs"));
    }

    #[test]
    fn test_missing_openrouter_key_error() {
        let config = AppConfig::default();
        let err = config
            .get_model_provider_config("some-model", "main chat")
            .unwrap_err();
        assert!(err.contains("OpenRouter"));
    }

}
