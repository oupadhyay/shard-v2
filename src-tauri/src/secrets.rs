/// Secrets module for secure API key storage using OS keyring
///
/// This module wraps the keyring crate to provide secure storage
/// for API keys using the OS-level secret manager:
/// - macOS: Keychain
/// - Linux: Secret Service (GNOME Keyring, KWallet)
/// - Windows: Credential Manager
///
/// All keys are stored in a single JSON entry to minimize password prompts.
/// An in-memory cache prevents redundant keychain reads within a session.
use keyring::Entry;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

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
    /// Brave Search API key
    Brave,
    /// Groq API key
    Groq,
}

impl ApiKeyType {
    /// Get the JSON key name for this key type
    pub(crate) fn key_name(&self) -> &'static str {
        match self {
            ApiKeyType::OpenAI => "api_key",
            ApiKeyType::Gemini => "gemini_api_key",
            ApiKeyType::OpenRouter => "openrouter_api_key",
            ApiKeyType::Brave => "brave_api_key",
            ApiKeyType::Groq => "groq_api_key",
        }
    }
}

/// Internal storage structure for all API keys
type ApiKeysStore = HashMap<String, String>;

/// In-memory cache to avoid redundant keychain reads.
///
/// The Mutex wraps an Option so we can invalidate the cache on writes
/// without replacing the lock itself. `None` means "not yet loaded."
static KEYS_CACHE: OnceLock<Mutex<Option<ApiKeysStore>>> = OnceLock::new();

fn cache() -> &'static Mutex<Option<ApiKeysStore>> {
    KEYS_CACHE.get_or_init(|| Mutex::new(None))
}

/// Load all API keys from keyring, using the in-memory cache when possible.
///
/// The first call pays the full keychain cost (one OS prompt on first launch).
/// Subsequent calls return the cached map without touching the keyring.
fn load_all_keys() -> Result<ApiKeysStore, String> {
    let mut guard = cache().lock().unwrap();

    if let Some(ref cached) = *guard {
        return Ok(cached.clone());
    }

    // Cache miss: go to the keyring exactly once.
    let entry = Entry::new(SERVICE_NAME, ENTRY_NAME)
        .map_err(|e| format!("Failed to create keyring entry: {}", e))?;

    let store = match entry.get_password() {
        Ok(json) => serde_json::from_str::<ApiKeysStore>(&json)
            .map_err(|e| format!("Failed to parse keyring data: {}", e))?,
        Err(keyring::Error::NoEntry) => HashMap::new(),
        Err(e) => return Err(format!("Failed to read keyring: {}", e)),
    };

    *guard = Some(store.clone());
    Ok(store)
}

/// Save all API keys to keyring and update the cache atomically.
fn save_all_keys(keys: &ApiKeysStore) -> Result<(), String> {
    let entry = Entry::new(SERVICE_NAME, ENTRY_NAME)
        .map_err(|e| format!("Failed to create keyring entry: {}", e))?;

    if keys.is_empty() {
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => {}
            Err(e) => return Err(format!("Failed to delete keyring entry: {}", e)),
        }
    } else {
        let json =
            serde_json::to_string(keys).map_err(|e| format!("Failed to serialize keys: {}", e))?;
        entry
            .set_password(&json)
            .map_err(|e| format!("Failed to save keyring: {}", e))?;
    }

    // Update cache to match the persisted state.
    *cache().lock().unwrap() = Some(keys.clone());
    Ok(())
}

/// Store multiple secrets in the OS keyring in a single operation.
///
/// Use this in batch operations (like migration or save_config) to avoid
/// multiple OS-level password prompts.
pub fn store_secrets_batch(updates: HashMap<ApiKeyType, Option<String>>) -> Result<(), String> {
    let mut keys = load_all_keys()?;
    let mut changed = false;

    for (key_type, value_opt) in updates {
        match value_opt {
            Some(value) if !value.is_empty() => {
                let name = key_type.key_name().to_string();
                if keys.get(&name) != Some(&value) {
                    keys.insert(name, value);
                    changed = true;
                }
            }
            Some(_) => {
                // Some("") means delete
                if keys.remove(key_type.key_name()).is_some() {
                    changed = true;
                }
            }
            None => {
                // None means "no change" or "don't update"
            }
        }
    }

    if changed {
        save_all_keys(&keys)?;
    }
    Ok(())
}

/// Retrieve all secrets at once, returning the full map.
///
/// Use this when you need multiple keys to avoid separate round-trips
/// (the cache makes them cheap, but this is cleaner).
pub fn get_all_secrets() -> Result<ApiKeysStore, String> {
    load_all_keys()
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
