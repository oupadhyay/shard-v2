/**
 * Tool Call Cache Module
 *
 * Provides TTL-based caching for tool results to reduce API load.
 * Each tool type has its own expiration time:
 * - web_search, search_wikipedia, search_arxiv: 7 days
 * - get_weather, get_stock_price: 1 hour
 * - Other tools: not cached
 */
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use tauri::{AppHandle, Manager, Runtime};

/// Cache entry with value and expiration time
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    pub value: String,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

/// Tool cache stored on disk
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolCache {
    /// Map of cache key (tool_name:args_hash) to cached result
    pub entries: HashMap<String, CacheEntry>,
}

/// Per-tool TTL configuration (in seconds)
pub fn get_ttl_for_tool(tool_name: &str) -> Option<i64> {
    match tool_name {
        // Long TTL (7 days) - relatively stable data
        "web_search" => Some(7 * 24 * 60 * 60),       // 7 days
        "search_wikipedia" => Some(7 * 24 * 60 * 60), // 7 days
        "search_arxiv" => Some(7 * 24 * 60 * 60),     // 7 days
        "read_arxiv_paper" => Some(7 * 24 * 60 * 60), // 7 days

        // Short TTL (1 hour) - frequently changing data
        "get_weather" => Some(60 * 60),      // 1 hour
        "get_stock_price" => Some(60 * 60),  // 1 hour

        // Not cached
        "save_memory" | "update_topic_summary" | "read_topic_summary" | "refresh_memories" => None,

        // Default: don't cache unknown tools
        _ => None,
    }
}

/// Generate a cache key from tool name and arguments
pub fn make_cache_key(tool_name: &str, args: &serde_json::Value) -> String {
    // Sort args for consistent hashing
    let args_str = serde_json::to_string(args).unwrap_or_default();
    let hash = seahash_str(&args_str);
    format!("{}:{:x}", tool_name, hash)
}

/// Simple hash function for argument strings
fn seahash_str(s: &str) -> u64 {
    // Simple FNV-1a hash for portability
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in s.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Get the cache file path
fn get_cache_path<R: Runtime>(app_handle: &AppHandle<R>) -> Result<PathBuf, String> {
    let app_data_dir = app_handle
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to get app data dir: {}", e))?;
    Ok(app_data_dir.join("tool_cache.json"))
}

/// Load the tool cache from disk
pub fn load_cache<R: Runtime>(app_handle: &AppHandle<R>) -> ToolCache {
    match get_cache_path(app_handle) {
        Ok(path) if path.exists() => {
            fs::read_to_string(&path)
                .ok()
                .and_then(|content| serde_json::from_str(&content).ok())
                .unwrap_or_default()
        }
        _ => ToolCache::default(),
    }
}

/// Save the tool cache to disk
fn save_cache<R: Runtime>(app_handle: &AppHandle<R>, cache: &ToolCache) {
    if let Ok(path) = get_cache_path(app_handle) {
        if let Ok(content) = serde_json::to_string_pretty(cache) {
            let _ = fs::write(&path, content);
        }
    }
}

/// Try to get a cached result for a tool call
/// Returns Some(result) if cache hit and not expired, None otherwise
pub fn get_cached_result<R: Runtime>(
    app_handle: &AppHandle<R>,
    tool_name: &str,
    args: &serde_json::Value,
) -> Option<String> {
    // Check if this tool is cacheable
    if get_ttl_for_tool(tool_name).is_none() {
        return None;
    }

    let cache = load_cache(app_handle);
    let key = make_cache_key(tool_name, args);

    if let Some(entry) = cache.entries.get(&key) {
        if entry.expires_at > Utc::now() {
            log::debug!("[Cache] HIT for {} (expires {})", key, entry.expires_at);
            return Some(entry.value.clone());
        } else {
            log::debug!("[Cache] EXPIRED for {}", key);
        }
    }

    None
}

/// Cache a tool result
pub fn cache_result<R: Runtime>(
    app_handle: &AppHandle<R>,
    tool_name: &str,
    args: &serde_json::Value,
    result: &str,
) {
    // Check if this tool is cacheable
    let Some(ttl_seconds) = get_ttl_for_tool(tool_name) else {
        return;
    };

    let mut cache = load_cache(app_handle);
    let key = make_cache_key(tool_name, args);
    let now = Utc::now();

    // Clean up expired entries while we're here (keep cache size manageable)
    cache.entries.retain(|_, entry| entry.expires_at > now);

    // Add new entry
    cache.entries.insert(
        key.clone(),
        CacheEntry {
            value: result.to_string(),
            expires_at: now + Duration::seconds(ttl_seconds),
            created_at: now,
        },
    );

    log::debug!(
        "[Cache] STORED {} (TTL {} seconds, {} total entries)",
        key,
        ttl_seconds,
        cache.entries.len()
    );

    save_cache(app_handle, &cache);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_key_consistency() {
        let args1 = serde_json::json!({"query": "test"});
        let args2 = serde_json::json!({"query": "test"});
        let args3 = serde_json::json!({"query": "different"});

        let key1 = make_cache_key("web_search", &args1);
        let key2 = make_cache_key("web_search", &args2);
        let key3 = make_cache_key("web_search", &args3);

        assert_eq!(key1, key2, "Same args should produce same key");
        assert_ne!(key1, key3, "Different args should produce different key");
    }

    #[test]
    fn test_ttl_configuration() {
        assert_eq!(get_ttl_for_tool("web_search"), Some(7 * 24 * 60 * 60));
        assert_eq!(get_ttl_for_tool("get_weather"), Some(60 * 60));
        assert_eq!(get_ttl_for_tool("save_memory"), None);
        assert_eq!(get_ttl_for_tool("unknown_tool"), None);
    }

    #[test]
    fn test_hash_determinism() {
        let hash1 = seahash_str("test string");
        let hash2 = seahash_str("test string");
        let hash3 = seahash_str("different string");

        assert_eq!(hash1, hash2);
        assert_ne!(hash1, hash3);
    }
}
