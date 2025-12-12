use crate::agent::{FunctionDefinition, ToolDefinition};
use serde_json::json;

pub fn get_all_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "get_weather".to_string(),
                description: "Get current weather for a location. Returns temperature, conditions, and humidity.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "location": { "type": "string", "description": "City name (e.g. 'Paris', 'London') or Zip code (e.g. '94102')" },
                    },
                    "required": ["location"]
                }),
            },
        },
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "search_wikipedia".to_string(),
                description: "Search Wikipedia for encyclopedic/historical information. Best for background knowledge, biographies, and established facts. NOT for current events, live scores, or breaking news.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Wikipedia article title. Use exact page title as it appears on Wikipedia (e.g., 'San Francisco 49ers', 'Albert Einstein')." },
                    },
                    "required": ["query"]
                }),
            },
        },
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "get_stock_price".to_string(),
                description: "Get current stock price and basic financial data for a ticker symbol. Returns price, change, and volume.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "symbol": { "type": "string", "description": "Stock ticker symbol, e.g. AAPL, GOOGL, MSFT" },
                    },
                    "required": ["symbol"]
                }),
            },
        },
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "search_arxiv".to_string(),
                description: "Search ArXiv for academic papers and preprints. Best for scientific research, AI/ML papers, physics, math. Returns paper titles, authors, and abstracts.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Academic search query, e.g. 'transformer attention mechanism' or 'quantum computing'" },
                    },
                    "required": ["query"]
                }),
            },
        },
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "web_search".to_string(),
                description: "Search the web for current/recent information. BEST for: sports scores, news, current events, live data, recent updates. Returns 5 results with title, URL, and snippet. One search is usually sufficient - avoid multiple redundant searches.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Search query. Be specific: include year, team name, 'current', 'latest', or 'today' for time-sensitive queries." },
                    },
                    "required": ["query"]
                }),
            },
        },
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "save_memory".to_string(),
                description: "Save important user preferences, context, or facts to persistent memory. Use for genuinely persistent information. Call when: user explicitly requests you remember something, user states a strong preference (language, units, coding style), or user provides important project context for ongoing work.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "category": {
                            "type": "string",
                            "enum": ["preference", "project", "interaction", "fact"],
                            "description": "Category of memory: 'preference' for user preferences, 'project' for project context, 'interaction' for conversation summaries, 'fact' for general facts about the user"
                        },
                        "content": { "type": "string", "description": "The information to remember. Be concise but complete." },
                        "importance": { "type": "integer", "minimum": 1, "maximum": 5, "description": "Importance level 1-5 (5=critical, 1=nice-to-have)" }
                    },
                    "required": ["category", "content", "importance"]
                }),
            },
        },
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "update_topic_summary".to_string(),
                description: "Create or update a focused summary file for a specific topic or project (e.g., 'SHARD', 'FINANCE'). Use this to consolidate scattered information into a single coherent document. IMPORTANT: Always use read_topic_summary first to get the existing content before updating.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "topic": { "type": "string", "description": "Topic name (e.g., 'SHARD', 'FINANCE'). Will be used as filename (SHARD.md)." },
                        "content": { "type": "string", "description": "The full markdown content of the summary. This overwrites the existing file, so ensure you include all relevant previous information plus new updates." },
                    },
                    "required": ["topic", "content"]
                }),
            },
        },
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "read_topic_summary".to_string(),
                description: "Read the content of an existing topic summary file. Use this before updating a summary to ensure you don't overwrite existing information.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "topic": { "type": "string", "description": "Topic name (e.g., 'SHARD', 'FINANCE')." },
                    },
                    "required": ["topic"]
                }),
            },
        },
    ]
}
