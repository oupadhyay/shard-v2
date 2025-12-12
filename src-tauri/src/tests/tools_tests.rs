#[cfg(test)]
mod tests {
    use crate::tools::get_all_tools;

    #[test]
    fn test_get_all_tools() {
        let tools = get_all_tools();
        assert!(!tools.is_empty());
        assert!(tools.len() >= 5);

        let tool_names: Vec<String> = tools.iter().map(|t| t.function.name.clone()).collect();
        assert!(tool_names.contains(&"get_weather".to_string()));
        assert!(tool_names.contains(&"search_wikipedia".to_string()));
        assert!(tool_names.contains(&"get_stock_price".to_string()));
        assert!(tool_names.contains(&"search_arxiv".to_string()));
        assert!(tool_names.contains(&"web_search".to_string()));
    }

    #[test]
    fn test_tool_structure() {
        let tools = get_all_tools();
        let weather_tool = tools.iter().find(|t| t.function.name == "get_weather").unwrap();

        assert_eq!(weather_tool.tool_type, "function");
        assert!(weather_tool.function.description.contains("weather"));

        let params = &weather_tool.function.parameters;
        assert!(params.get("type").is_some());
        assert!(params.get("properties").is_some());
        assert!(params.get("required").is_some());
    }
}
