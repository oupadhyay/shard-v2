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
    pub brave_api_key: Option<String>,
    pub selected_model: Option<String>,
    pub api_base_url: Option<String>, // e.g., https://generativelanguage.googleapis.com/v1beta/openai/
    pub enable_web_search: Option<bool>,
    pub enable_tools: Option<bool>,
    pub system_prompt: Option<String>, // Custom system prompt, if None will use MCP default
    pub jailbreak_mode: Option<bool>,
    pub research_mode: Option<bool>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            api_key: None,
            gemini_api_key: None,
            openrouter_api_key: None,
            brave_api_key: None,
            selected_model: None,
            api_base_url: None,
            enable_web_search: None,
            enable_tools: Some(true),
            system_prompt: None,
            jailbreak_mode: None,
            research_mode: Some(false),
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
    toml::from_str(&content).map_err(|e| format!("Failed to parse config file: {}", e))
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
