#[cfg(test)]
mod tests {
    use crate::tool_registry::ToolRegistry;

    #[test]
    fn test_registry_has_all_tools() {
        let reg = ToolRegistry::new();
        // Should have all tools registered
        assert!(reg.get("web_search").is_some());
        assert!(reg.get("open_url").is_some());
        assert!(reg.get("search_wikipedia").is_some());
        assert!(reg.get("youtube_transcript").is_some());
        assert!(reg.get("search_arxiv").is_some());
        assert!(reg.get("read_arxiv_paper").is_some());
        assert!(reg.get("get_stock_price").is_some());
        assert!(reg.get("get_weather").is_some());
        assert!(reg.get("save_memory").is_some());
        assert!(reg.get("update_topic_summary").is_some());
        assert!(reg.get("read_topic_summary").is_some());
        assert!(reg.get("refresh_memories").is_some());
        assert!(reg.get("memory_search").is_some());
        assert!(reg.get("memory_get").is_some());
        assert!(reg.get("load_persona").is_some());
        assert!(reg.get("unload_persona").is_some());
        assert!(reg.get("list_personas").is_some());
        assert!(reg.get("run_python").is_some());
        assert!(reg.get("wake_me_up_in").is_some());
        assert!(reg.get("edit_config").is_some());
        assert!(reg.get("create_heartbeat").is_some());
        assert!(reg.get("delete_heartbeat").is_some());
        assert!(reg.get("edit_heartbeat").is_some());
    }

    #[test]
    fn test_unknown_tool_returns_none() {
        let reg = ToolRegistry::new();
        assert!(reg.get("nonexistent_tool").is_none());
    }

    #[test]
    fn test_get_definitions_excludes_draft_gated() {
        let reg = ToolRegistry::new();
        let defs = reg.get_definitions(&[]);
        let names: Vec<String> = defs.iter().map(|d| d.function.name.clone()).collect();

        // Draft-gated tools should NOT appear in normal definitions
        assert!(!names.contains(&"edit_config".to_string()));
        assert!(!names.contains(&"create_heartbeat".to_string()));
        assert!(!names.contains(&"delete_heartbeat".to_string()));
        assert!(!names.contains(&"edit_heartbeat".to_string()));

        // Global tools should appear
        assert!(names.contains(&"web_search".to_string()));
        assert!(names.contains(&"memory_search".to_string()));
        assert!(names.contains(&"run_python".to_string()));
    }

    #[test]
    fn test_get_definitions_excludes_non_global_by_default() {
        let reg = ToolRegistry::new();
        let defs = reg.get_definitions(&[]);
        let names: Vec<String> = defs.iter().map(|d| d.function.name.clone()).collect();

        // Non-global tools (persona-gated) should NOT appear without a persona
        assert!(!names.contains(&"get_weather".to_string()));
        assert!(!names.contains(&"get_stock_price".to_string()));
        assert!(!names.contains(&"search_arxiv".to_string()));
    }

    #[test]
    fn test_heartbeat_definitions_include_draft_gated() {
        let reg = ToolRegistry::new();
        let defs = reg.get_heartbeat_definitions(&[]);
        let names: Vec<String> = defs.iter().map(|d| d.function.name.clone()).collect();

        // Draft-gated tools should appear in heartbeat definitions
        assert!(names.contains(&"edit_config".to_string()));
        assert!(names.contains(&"create_heartbeat".to_string()));
        assert!(names.contains(&"delete_heartbeat".to_string()));
        assert!(names.contains(&"edit_heartbeat".to_string()));

        // Plus global tools
        assert!(names.contains(&"web_search".to_string()));
    }

    #[test]
    fn test_parity_with_old_get_all_tools() {
        // Verify that the registry produces the same global tool set as the old tools.rs
        let reg = ToolRegistry::new();
        let registry_defs = reg.get_definitions(&[]);
        let old_defs = crate::tools::get_all_tools(&[]);

        let mut registry_names: Vec<String> =
            registry_defs.iter().map(|d| d.function.name.clone()).collect();
        let mut old_names: Vec<String> =
            old_defs.iter().map(|d| d.function.name.clone()).collect();

        registry_names.sort();
        old_names.sort();

        assert_eq!(
            registry_names, old_names,
            "Registry definitions should match old get_all_tools() for no-persona case"
        );
    }

    #[test]
    fn test_parity_with_old_get_heartbeat_tools() {
        let reg = ToolRegistry::new();
        let registry_defs = reg.get_heartbeat_definitions(&[]);
        let old_defs = crate::tools::get_heartbeat_tools(&[]);

        let mut registry_names: Vec<String> =
            registry_defs.iter().map(|d| d.function.name.clone()).collect();
        let mut old_names: Vec<String> =
            old_defs.iter().map(|d| d.function.name.clone()).collect();

        registry_names.sort();
        old_names.sort();

        assert_eq!(
            registry_names, old_names,
            "Registry heartbeat definitions should match old get_heartbeat_tools()"
        );
    }

    // ── Toolset grouping ─────────────────────────────────────────────

    #[test]
    fn test_toolsets_are_populated() {
        let reg = ToolRegistry::new();
        let toolsets = reg.toolsets();

        assert!(toolsets.contains(&"web"));
        assert!(toolsets.contains(&"memory"));
        assert!(toolsets.contains(&"persona"));
        assert!(toolsets.contains(&"research"));
        assert!(toolsets.contains(&"code"));
        assert!(toolsets.contains(&"automation"));
    }

    #[test]
    fn test_tools_in_toolset() {
        let reg = ToolRegistry::new();
        let web_tools = reg.tools_in_toolset("web");

        assert!(web_tools.contains(&"web_search"));
        assert!(web_tools.contains(&"open_url"));
        assert!(web_tools.contains(&"search_wikipedia"));
        assert!(web_tools.contains(&"youtube_transcript"));
        // Non-web tools should NOT be in this set
        assert!(!web_tools.contains(&"save_memory"));
    }

    // ── Parallel safety ──────────────────────────────────────────────

    #[test]
    fn test_parallel_safe_tools() {
        let reg = ToolRegistry::new();

        // Read-only tools should be parallel safe
        assert!(reg.get("web_search").unwrap().parallel_safe);
        assert!(reg.get("open_url").unwrap().parallel_safe);
        assert!(reg.get("memory_search").unwrap().parallel_safe);
        assert!(reg.get("search_wikipedia").unwrap().parallel_safe);
        assert!(reg.get("read_topic_summary").unwrap().parallel_safe);

        // Write/mutating tools should NOT be parallel safe
        assert!(!reg.get("save_memory").unwrap().parallel_safe);
        assert!(!reg.get("update_topic_summary").unwrap().parallel_safe);
        assert!(!reg.get("load_persona").unwrap().parallel_safe);
        assert!(!reg.get("run_python").unwrap().parallel_safe);
    }

    #[test]
    fn test_should_parallelize_batch() {
        let reg = ToolRegistry::new();

        // All parallel-safe → true
        assert!(reg.should_parallelize(&["web_search", "search_wikipedia", "open_url"]));

        // One non-parallel → false
        assert!(!reg.should_parallelize(&["web_search", "save_memory"]));

        // Empty → true (vacuous truth)
        assert!(reg.should_parallelize(&[]));
    }

    // ── Cache TTL ────────────────────────────────────────────────────

    #[test]
    fn test_cache_ttl_matches_old_module() {
        let reg = ToolRegistry::new();

        // 7-day tools
        assert_eq!(reg.cache_ttl("web_search"), Some(7 * 24 * 60 * 60));
        assert_eq!(reg.cache_ttl("search_wikipedia"), Some(7 * 24 * 60 * 60));
        assert_eq!(reg.cache_ttl("search_arxiv"), Some(7 * 24 * 60 * 60));
        assert_eq!(reg.cache_ttl("read_arxiv_paper"), Some(7 * 24 * 60 * 60));

        // 60-day tools
        assert_eq!(reg.cache_ttl("youtube_transcript"), Some(60 * 24 * 60 * 60));

        // 1-hour tools
        assert_eq!(reg.cache_ttl("get_weather"), Some(60 * 60));
        assert_eq!(reg.cache_ttl("get_stock_price"), Some(60 * 60));

        // No-cache tools
        assert_eq!(reg.cache_ttl("save_memory"), None);
        assert_eq!(reg.cache_ttl("update_topic_summary"), None);
        assert_eq!(reg.cache_ttl("refresh_memories"), None);
        assert_eq!(reg.cache_ttl("run_python"), None);
    }

    // ── Draft gating ─────────────────────────────────────────────────

    #[test]
    fn test_is_draft_gated() {
        let reg = ToolRegistry::new();

        assert!(reg.is_draft_gated("edit_config"));
        assert!(reg.is_draft_gated("create_heartbeat"));
        assert!(reg.is_draft_gated("delete_heartbeat"));
        assert!(reg.is_draft_gated("edit_heartbeat"));

        assert!(!reg.is_draft_gated("web_search"));
        assert!(!reg.is_draft_gated("save_memory"));
        assert!(!reg.is_draft_gated("run_python"));
        assert!(!reg.is_draft_gated("wake_me_up_in"));
    }

    // ── Schema structure ─────────────────────────────────────────────

    #[test]
    fn test_tool_schema_structure() {
        let reg = ToolRegistry::new();
        let entry = reg.get("web_search").unwrap();

        assert_eq!(entry.schema.tool_type, "function");
        assert_eq!(entry.schema.function.name, "web_search");
        assert!(entry.schema.function.description.contains("web"));

        let params = &entry.schema.function.parameters;
        assert_eq!(params["type"], "object");
        assert!(params["properties"].get("query").is_some());
        assert!(params["required"].as_array().unwrap().contains(&serde_json::json!("query")));
    }
}
