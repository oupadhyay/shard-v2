/// Secrets module for secure API key storage using OS keyring
///
/// This module wraps the keyring crate to provide secure storage
/// for API keys using the OS-level secret manager:
/// - macOS: Keychain
/// - Linux: Secret Service (GNOME Keyring, KWallet)
/// - Windows: Credential Manager

use keyring::Entry;

/// Service name for all Shard secrets in the OS keyring
const SERVICE_NAME: &str = "dev.shard.app";

/// Supported API key types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    /// Get the keyring entry name for this key type
    fn entry_name(&self) -> &'static str {
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

/// Store a secret in the OS keyring
///
/// # Arguments
/// * `key_type` - The type of API key to store
/// * `value` - The secret value to store
///
/// # Returns
/// * `Ok(())` if the secret was stored successfully
/// * `Err(String)` if storage failed
pub fn store_secret(key_type: ApiKeyType, value: &str) -> Result<(), String> {
    if value.is_empty() {
        // Don't store empty values, just delete any existing entry
        return delete_secret(key_type);
    }

    log::info!("[Secrets] Creating entry for service='{}', account='{}'", SERVICE_NAME, key_type.entry_name());

    let entry = Entry::new(SERVICE_NAME, key_type.entry_name())
        .map_err(|e| {
            log::error!("[Secrets] Failed to create entry: {}", e);
            format!("Failed to create keyring entry: {}", e)
        })?;

    entry
        .set_password(value)
        .map_err(|e| {
            log::error!("[Secrets] Failed to set password: {}", e);
            format!("Failed to store secret in keyring: {}", e)
        })?;

    log::info!("[Secrets] Successfully stored {:?}", key_type);
    Ok(())
}

/// Retrieve a secret from the OS keyring
///
/// # Arguments
/// * `key_type` - The type of API key to retrieve
///
/// # Returns
/// * `Ok(Some(String))` if the secret was found
/// * `Ok(None)` if no secret exists for this key type
/// * `Err(String)` if retrieval failed
pub fn get_secret(key_type: ApiKeyType) -> Result<Option<String>, String> {
    log::debug!("[Secrets] Getting {:?} from keyring", key_type);

    let entry = Entry::new(SERVICE_NAME, key_type.entry_name())
        .map_err(|e| {
            log::error!("[Secrets] Failed to create entry for get: {}", e);
            format!("Failed to create keyring entry: {}", e)
        })?;

    match entry.get_password() {
        Ok(password) => {
            log::info!("[Secrets] Found {:?} (len={})", key_type, password.len());
            Ok(Some(password))
        }
        Err(keyring::Error::NoEntry) => {
            log::debug!("[Secrets] No entry for {:?}", key_type);
            Ok(None)
        }
        Err(e) => {
            log::error!("[Secrets] Failed to get {:?}: {}", key_type, e);
            Err(format!("Failed to retrieve secret from keyring: {}", e))
        }
    }
}

/// Delete a secret from the OS keyring
///
/// # Arguments
/// * `key_type` - The type of API key to delete
///
/// # Returns
/// * `Ok(())` if the secret was deleted or didn't exist
/// * `Err(String)` if deletion failed
pub fn delete_secret(key_type: ApiKeyType) -> Result<(), String> {
    let entry = Entry::new(SERVICE_NAME, key_type.entry_name())
        .map_err(|e| format!("Failed to create keyring entry: {}", e))?;

    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()), // Already deleted, that's fine
        Err(e) => Err(format!("Failed to delete secret from keyring: {}", e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: These tests interact with the real OS keyring
    // They use a unique test service name to avoid conflicts

    #[test]
    fn test_api_key_type_entry_names() {
        assert_eq!(ApiKeyType::OpenAI.entry_name(), "api_key");
        assert_eq!(ApiKeyType::Gemini.entry_name(), "gemini_api_key");
        assert_eq!(ApiKeyType::OpenRouter.entry_name(), "openrouter_api_key");
    }
}
