/**
 * Cache Module Tests
 *
 * Tests for the tool call caching system.
 */

#[cfg(test)]
mod tests {
    use crate::cache::{get_ttl_for_tool, make_cache_key};

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
    fn test_cache_key_different_tools() {
        let args = serde_json::json!({"query": "test"});

        let key1 = make_cache_key("web_search", &args);
        let key2 = make_cache_key("search_wikipedia", &args);

        assert_ne!(key1, key2, "Different tools should produce different keys");
    }

    #[test]
    fn test_ttl_long_duration_tools() {
        // 7 days = 604800 seconds
        assert_eq!(get_ttl_for_tool("web_search"), Some(604800));
        assert_eq!(get_ttl_for_tool("search_wikipedia"), Some(604800));
        assert_eq!(get_ttl_for_tool("search_arxiv"), Some(604800));
        assert_eq!(get_ttl_for_tool("read_arxiv_paper"), Some(604800));
    }

    #[test]
    fn test_ttl_short_duration_tools() {
        // 1 hour = 3600 seconds
        assert_eq!(get_ttl_for_tool("get_weather"), Some(3600));
        assert_eq!(get_ttl_for_tool("get_stock_price"), Some(3600));
    }

    #[test]
    fn test_ttl_non_cached_tools() {
        assert_eq!(get_ttl_for_tool("save_memory"), None);
        assert_eq!(get_ttl_for_tool("update_topic_summary"), None);
        assert_eq!(get_ttl_for_tool("read_topic_summary"), None);
        assert_eq!(get_ttl_for_tool("refresh_memories"), None);
        assert_eq!(get_ttl_for_tool("unknown_tool"), None);
    }

    #[test]
    fn test_cache_key_format() {
        let args = serde_json::json!({"query": "rust programming"});
        let key = make_cache_key("web_search", &args);

        // Key should be in format "tool_name:hex_hash"
        assert!(key.starts_with("web_search:"));
        assert!(key.len() > "web_search:".len());
    }

    #[test]
    fn test_cache_key_empty_args() {
        let args = serde_json::json!({});
        let key = make_cache_key("refresh_memories", &args);

        assert!(key.starts_with("refresh_memories:"));
    }
}
