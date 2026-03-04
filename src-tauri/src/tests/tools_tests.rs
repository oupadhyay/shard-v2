#[cfg(test)]
mod tests {
    use crate::tools::get_all_tools;

    #[test]
    fn test_get_all_tools() {
        let tools = get_all_tools(&[]);
        assert!(!tools.is_empty());

        let tool_names: Vec<String> = tools.iter().map(|t| t.function.name.clone()).collect();
        // Domain specific tools should NOT be loaded by default
        assert!(!tool_names.contains(&"get_weather".to_string()));
        assert!(!tool_names.contains(&"get_stock_price".to_string()));
        assert!(!tool_names.contains(&"search_arxiv".to_string()));

        // Global tools should be loaded
        assert!(tool_names.contains(&"search_wikipedia".to_string()));
        assert!(tool_names.contains(&"web_search".to_string()));
        assert!(tool_names.contains(&"refresh_memories".to_string()));
        assert!(tool_names.contains(&"memory_search".to_string()));
        assert!(tool_names.contains(&"memory_get".to_string()));
    }

    #[test]
    fn test_tool_structure() {
        let tools = get_all_tools(&[]);
        let web_search_tool = tools
            .iter()
            .find(|t| t.function.name == "web_search")
            .unwrap();

        assert_eq!(web_search_tool.tool_type, "function");
        assert!(web_search_tool.function.description.contains("web"));

        let params = &web_search_tool.function.parameters;
        assert!(params.get("type").is_some());
        assert!(params.get("properties").is_some());
        assert!(params.get("required").is_some());
    }
}
