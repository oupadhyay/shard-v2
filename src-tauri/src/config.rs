use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use tauri::{AppHandle, Manager, Runtime};

const CONFIG_FILENAME: &str = "config.toml";

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AppConfig {
    pub api_key: Option<String>, // Generic/OpenAI key
    pub gemini_api_key: Option<String>,
    pub openrouter_api_key: Option<String>,
    pub cerebras_api_key: Option<String>,
    pub brave_api_key: Option<String>,
    pub selected_model: Option<String>,
    pub api_base_url: Option<String>, // e.g., https://generativelanguage.googleapis.com/v1beta/openai/
    pub enable_web_search: Option<bool>,
    pub enable_tools: Option<bool>,
    pub system_prompt: Option<String>, // Custom system prompt, if None will use MCP default
    pub incognito_mode: Option<bool>,
    pub research_mode: Option<bool>,
    pub groq_api_key: Option<String>,
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
            compaction_threshold: Some(0.001),
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
    let config_path = get_config_path(app_handle)?;
    if !config_path.exists() {
        return Ok(AppConfig::default());
    }
    let content = fs::read_to_string(&config_path)
        .map_err(|e| format!("Failed to read config file: {}", e))?;
    let mut loaded: AppConfig = toml::from_str(&content)
        .map_err(|e| format!("Failed to parse config file: {}", e))?;

    // Merge with defaults: if a field is None in loaded config, use Default impl value
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

pub fn save_config<R: Runtime>(app_handle: &AppHandle<R>, config: &AppConfig) -> Result<(), String> {
    let config_path = get_config_path(app_handle)?;
    if let Some(parent_dir) = config_path.parent() {
        if !parent_dir.exists() {
            fs::create_dir_all(parent_dir)
                .map_err(|e| format!("Failed to create config directory: {}", e))?;
        }
    }
    let toml_string =
        toml::to_string_pretty(config).map_err(|e| format!("Failed to serialize config: {}", e))?;
    fs::write(&config_path, toml_string).map_err(|e| format!("Failed to write config file: {}", e))
}
