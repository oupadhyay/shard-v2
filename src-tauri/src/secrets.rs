/// Secrets module for secure API key storage using OS keyring
///
/// This module wraps the keyring crate to provide secure storage
/// for API keys using the OS-level secret manager:
/// - macOS: Keychain
/// - Linux: Secret Service (GNOME Keyring, KWallet)
/// - Windows: Credential Manager
///
/// All keys are stored in a single JSON entry to minimize password prompts.

use keyring::Entry;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Service name for all Shard secrets in the OS keyring
const SERVICE_NAME: &str = "dev.shard.app";
/// Single entry name for all API keys (stored as JSON)
const ENTRY_NAME: &str = "api_keys";

/// Supported API key types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiKeyType {
    /// Generic OpenAI-compatible API key
    OpenAI,
    /// Google Gemini API key
    Gemini,
    /// OpenRouter API key
    OpenRouter,
    /// Cerebras API key
    Cerebras,
    /// Brave Search API key
    Brave,
    /// Groq API key
    Groq,
}

impl ApiKeyType {
    /// Get the JSON key name for this key type
    fn key_name(&self) -> &'static str {
        match self {
            ApiKeyType::OpenAI => "api_key",
            ApiKeyType::Gemini => "gemini_api_key",
            ApiKeyType::OpenRouter => "openrouter_api_key",
            ApiKeyType::Cerebras => "cerebras_api_key",
            ApiKeyType::Brave => "brave_api_key",
            ApiKeyType::Groq => "groq_api_key",
        }
    }
}

/// Internal storage structure for all API keys
type ApiKeysStore = HashMap<String, String>;

/// Load all API keys from keyring
fn load_all_keys() -> Result<ApiKeysStore, String> {
    let entry = Entry::new(SERVICE_NAME, ENTRY_NAME)
        .map_err(|e| format!("Failed to create keyring entry: {}", e))?;

    match entry.get_password() {
        Ok(json) => {
            serde_json::from_str(&json)
                .map_err(|e| format!("Failed to parse keyring data: {}", e))
        }
        Err(keyring::Error::NoEntry) => Ok(HashMap::new()),
        Err(e) => Err(format!("Failed to read keyring: {}", e)),
    }
}

/// Old entry names from the pre-consolidation format
const OLD_ENTRY_NAMES: &[(&str, ApiKeyType)] = &[
    ("api_key", ApiKeyType::OpenAI),
    ("gemini_api_key", ApiKeyType::Gemini),
    ("openrouter_api_key", ApiKeyType::OpenRouter),
    ("cerebras_api_key", ApiKeyType::Cerebras),
    ("brave_api_key", ApiKeyType::Brave),
    ("groq_api_key", ApiKeyType::Groq),
];

/// Migrate old separate keychain entries to new consolidated format.
/// Call this on app startup to handle users upgrading from old versions.
pub fn migrate_legacy_entries() {
    let mut migrated_keys: ApiKeysStore = HashMap::new();
    let mut entries_to_delete: Vec<Entry> = Vec::new();

    // First pass: collect all keys from old entries (don't delete yet)
    for (old_name, key_type) in OLD_ENTRY_NAMES {
        if let Ok(entry) = Entry::new(SERVICE_NAME, old_name) {
            if let Ok(password) = entry.get_password() {
                if !password.is_empty() {
                    log::info!("[Secrets] Found legacy entry: {}", old_name);
                    migrated_keys.insert(key_type.key_name().to_string(), password);
                    entries_to_delete.push(entry);
                }
            }
        }
    }

    if migrated_keys.is_empty() {
        return; // No legacy entries to migrate
    }

    // Merge with any existing consolidated keys
    if let Ok(existing) = load_all_keys() {
        for (k, v) in existing {
            migrated_keys.entry(k).or_insert(v);
        }
    }

    // Save the merged result - only proceed with deletion if this succeeds
    match save_all_keys(&migrated_keys) {
        Ok(()) => {
            log::info!("[Secrets] Migration saved: {} keys consolidated", migrated_keys.len());
            let count = entries_to_delete.len();
            // Now safe to delete old entries
            for entry in entries_to_delete {
                if let Err(e) = entry.delete_credential() {
                    if !matches!(e, keyring::Error::NoEntry) {
                        log::warn!("[Secrets] Failed to delete legacy entry: {}", e);
                    }
                }
            }
            log::info!("[Secrets] Migration complete: deleted {} legacy entries", count);
        }
        Err(e) => {
            log::error!("[Secrets] Migration failed, keeping legacy entries: {}", e);
            // Don't delete old entries - they're still the only copies!
        }
    }
}

/// Save all API keys to keyring
fn save_all_keys(keys: &ApiKeysStore) -> Result<(), String> {
    let entry = Entry::new(SERVICE_NAME, ENTRY_NAME)
        .map_err(|e| format!("Failed to create keyring entry: {}", e))?;

    if keys.is_empty() {
        // Delete entry if no keys
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(format!("Failed to delete keyring entry: {}", e)),
        }
    } else {
        let json = serde_json::to_string(keys)
            .map_err(|e| format!("Failed to serialize keys: {}", e))?;
        entry.set_password(&json)
            .map_err(|e| format!("Failed to save keyring: {}", e))
    }
}

/// Store a secret in the OS keyring
pub fn store_secret(key_type: ApiKeyType, value: &str) -> Result<(), String> {
    let mut keys = load_all_keys()?;

    if value.is_empty() {
        keys.remove(key_type.key_name());
        log::info!("[Secrets] Removed {:?} from keyring", key_type);
    } else {
        keys.insert(key_type.key_name().to_string(), value.to_string());
        log::info!("[Secrets] Stored {:?} (len={})", key_type, value.len());
    }

    save_all_keys(&keys)
}

/// Retrieve a secret from the OS keyring
pub fn get_secret(key_type: ApiKeyType) -> Result<Option<String>, String> {
    let keys = load_all_keys()?;
    let result = keys.get(key_type.key_name()).cloned();

    if let Some(ref v) = result {
        log::debug!("[Secrets] Found {:?} (len={})", key_type, v.len());
    }

    Ok(result)
}

/// Delete a secret from the OS keyring
pub fn delete_secret(key_type: ApiKeyType) -> Result<(), String> {
    store_secret(key_type, "")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_api_key_type_key_names() {
        assert_eq!(ApiKeyType::OpenAI.key_name(), "api_key");
        assert_eq!(ApiKeyType::Gemini.key_name(), "gemini_api_key");
        assert_eq!(ApiKeyType::OpenRouter.key_name(), "openrouter_api_key");
    }
}
