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
    pub max_auto_retries: Option<u32>,   // Default: 2
    pub retry_on_empty: Option<bool>,    // Retry empty responses after reasoning
    pub retry_on_katex: Option<bool>,    // Retry on frontend KaTeX parse errors
    // Screen context capture
    pub enable_screen_context: Option<bool>, // Default: false
    // Compaction configuration
    pub enable_compaction: Option<bool>,       // Default: true
    pub compaction_threshold: Option<f32>,     // Default: 0.5 (50%)
    pub compaction_preserve_turns: Option<u32>, // Default: 5
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

    // Migrate old separate keychain entries to consolidated format (one-time)
    secrets::migrate_legacy_entries();

    let config_path = get_config_path(app_handle)?;
    let mut loaded = if !config_path.exists() {
        AppConfig::default()
    } else {
        let content = fs::read_to_string(&config_path)
            .map_err(|e| format!("Failed to read config file: {}", e))?;
        toml::from_str::<AppConfig>(&content)
            .map_err(|e| format!("Failed to parse config file: {}", e))?
    };

    // Migrate any existing TOML keys to keyring (one-time operation)
    let mut needs_resave = false;
    let migrations = [
        (&loaded.api_key, ApiKeyType::OpenAI),
        (&loaded.gemini_api_key, ApiKeyType::Gemini),
        (&loaded.openrouter_api_key, ApiKeyType::OpenRouter),
        (&loaded.cerebras_api_key, ApiKeyType::Cerebras),
        (&loaded.brave_api_key, ApiKeyType::Brave),
        (&loaded.groq_api_key, ApiKeyType::Groq),
    ];

    for (toml_key, key_type) in migrations {
        if let Some(key) = toml_key {
            if !key.is_empty() {
                // Only migrate if keyring doesn't already have this key
                if secrets::get_secret(key_type).unwrap_or(None).is_none() {
                    if let Err(e) = secrets::store_secret(key_type, key) {
                        log::warn!("[Config] Failed to migrate {:?} to keyring: {}", key_type, e);
                    } else {
                        log::info!("[Config] Migrated {:?} from config.toml to keyring", key_type);
                    }
                }
                needs_resave = true;
            }
        }
    }

    // Load keys from keyring into config struct (for runtime use)
    loaded.api_key = secrets::get_secret(ApiKeyType::OpenAI).unwrap_or(None);
    loaded.gemini_api_key = secrets::get_secret(ApiKeyType::Gemini).unwrap_or(None);
    loaded.openrouter_api_key = secrets::get_secret(ApiKeyType::OpenRouter).unwrap_or(None);
    loaded.cerebras_api_key = secrets::get_secret(ApiKeyType::Cerebras).unwrap_or(None);
    loaded.brave_api_key = secrets::get_secret(ApiKeyType::Brave).unwrap_or(None);
    loaded.groq_api_key = secrets::get_secret(ApiKeyType::Groq).unwrap_or(None);

    // Re-save to remove migrated keys from TOML file
    if needs_resave {
        if let Err(e) = save_config_internal(&config_path, &loaded) {
            log::warn!("[Config] Failed to clean TOML after migration: {}", e);
        }
    }

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

pub fn save_config<R: Runtime>(app_handle: &AppHandle<R>, config: &AppConfig) -> Result<(), String> {
    use crate::secrets::{self, ApiKeyType};

    // Save API keys to keyring
    // Only update keyring when explicitly provided:
    // - Some(non-empty) -> store the key
    // - Some("") -> delete the key (user cleared it)
    // - None -> keep existing keyring value (field not provided in partial update)
    let key_saves = [
        (&config.api_key, ApiKeyType::OpenAI),
        (&config.gemini_api_key, ApiKeyType::Gemini),
        (&config.openrouter_api_key, ApiKeyType::OpenRouter),
        (&config.cerebras_api_key, ApiKeyType::Cerebras),
        (&config.brave_api_key, ApiKeyType::Brave),
        (&config.groq_api_key, ApiKeyType::Groq),
    ];

    for (key_value, key_type) in key_saves {
        match key_value {
            Some(value) if !value.is_empty() => {
                log::info!("[Config] Storing {:?} to keyring (len={})", key_type, value.len());
                secrets::store_secret(key_type, value)?;
            }
            Some(_) => {
                // Empty string = explicit delete
                log::info!("[Config] Deleting {:?} from keyring (empty value)", key_type);
                secrets::delete_secret(key_type)?;
            }
            None => {
                // None = not provided, keep existing keyring value
                log::debug!("[Config] Keeping existing {:?} (not provided)", key_type);
            }
        }
    }

    // Save non-sensitive config to TOML (API keys cleared before serialization)
    let config_path = get_config_path(app_handle)?;
    save_config_internal(&config_path, config)
}

