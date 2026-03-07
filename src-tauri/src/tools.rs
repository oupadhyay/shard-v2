use crate::agent::{FunctionDefinition, ToolDefinition};
use serde_json::json;

pub fn get_all_tools(active_skills: &[String]) -> Vec<ToolDefinition> {
    // Collect all required tools from the active skills
    let mut required_tools = std::collections::HashSet::new();
    for skill in active_skills {
        for tool in crate::skills::get_skill_required_tools(skill) {
            required_tools.insert(tool);
        }
    }

    // Global tools always available to the agent
    let global_tools = [
        "web_search",
        "open_url",
        "memory_search",
        "memory_get",
        "save_memory",
        "refresh_memories",
        "read_topic_summary",
        "update_topic_summary",
        "load_skill",
        "unload_skill",
        "list_skills",
        "search_wikipedia",
        "youtube_transcript",
    ];

    let all_tools = vec![
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "get_weather".to_string(),
                description: "Get current weather for a location. Returns temperature, conditions, and humidity.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "location": { "type": "string", "description": "City name ONLY (e.g. 'Paris', 'London', 'Tracy'). Do NOT include state abbreviations, country codes, or commas." },
                    },
                    "required": ["location"],
                    "additionalProperties": false
                }),
                strict: Some(true),
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
                        "query": { "type": "string", "description": "Wikipedia article title. Use exact page title as it appears on Wikipedia (e.g., 'San Francisco 49ers', 'Albert Einstein'). For example, use 'SchedMD' and 'NVIDIA' not 'SchedMD acquisition by NVIDIA'" },
                    },
                    "required": ["query"],
                    "additionalProperties": false
                }),
                strict: Some(true),
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
                    "required": ["symbol"],
                    "additionalProperties": false
                }),
                strict: Some(true),
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
                    "required": ["query"],
                    "additionalProperties": false
                }),
                strict: Some(true),
            },
        },
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "read_arxiv_paper".to_string(),
                description: "Read the full content of an ArXiv paper. Use this AFTER search_arxiv to get detailed paper content. Input can be ArXiv paper ID (e.g., '2401.12345') or URL.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "paper_id": { "type": "string", "description": "ArXiv paper ID (e.g., '2401.12345') or full arxiv.org URL" },
                    },
                    "required": ["paper_id"],
                    "additionalProperties": false
                }),
                strict: Some(true),
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
                    "required": ["query"],
                    "additionalProperties": false
                }),
                strict: Some(true),
            },
        },
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "open_url".to_string(),
                description: "Read the main text content of any URL/web page and provides clean, readable text. Use this to read specific articles or pages found via web_search or directly requested by the user.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "url": { "type": "string", "description": "The full HTTPS URL of the web page to read." },
                    },
                    "required": ["url"],
                    "additionalProperties": false
                }),
                strict: Some(true),
            },
        },
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "youtube_transcript".to_string(),
                description: "Get the transcript/captions of a YouTube video. Accepts a YouTube URL or video ID. Returns timestamped text. Use this when the user asks about the content of a YouTube video, wants a summary, or references a YouTube link.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "video": { "type": "string", "description": "YouTube video URL (e.g. 'https://www.youtube.com/watch?v=dQw4w9WgXcQ') or video ID (e.g. 'dQw4w9WgXcQ')" },
                    },
                    "required": ["video"],
                    "additionalProperties": false
                }),
                strict: Some(true),
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
                        "importance": { "type": "integer", "description": "Importance level 1-5 (5=critical, 1=nice-to-have)" }
                    },
                    "required": ["category", "content", "importance"],
                    "additionalProperties": false
                }),
                strict: Some(true),
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
                    "required": ["topic", "content"],
                    "additionalProperties": false
                }),
                strict: Some(true),
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
                    "required": ["topic"],
                    "additionalProperties": false
                }),
                strict: Some(true),
            },
        },
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "refresh_memories".to_string(),
                description: "Run background memory analysis to update topic summaries and insights from recent conversations. Use only when the user explicitly requests a memory update AND there is significant new information to remember.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                }),
                strict: Some(true),
            },
        },
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "memory_search".to_string(),
                description: "Search your persistent memory (topics, insights, session transcripts) using semantic and keyword search. Returns ranked snippets with source paths and line ranges. Can use temporal filters for chats.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Natural language search query describing what to find in memory" },
                        "max_results": { "type": "integer", "description": "Maximum number of results to return (default: 5)" },
                        "min_score": { "type": "number", "description": "Minimum similarity score 0.0-1.0 (default: 0.3)." },
                        "time_filter": { "type": "string", "description": "Optional: 'last_conversation', 'yesterday', 'last_week', or specific YYYY-MM-DD date" }
                    },
                    "required": ["query"],
                    "additionalProperties": false
                }),
                strict: Some(true),
            },
        },
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "memory_get".to_string(),
                description: "Read specific lines from a memory file or full session transcript. Provide either 'path' (for topics/insights) or 'session_id' (for full chats).".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Relative path to the memory file (e.g. 'topics/SHARD.md')" },
                        "session_id": { "type": "string", "description": "Session UUID to fetch the full transcript." },
                        "from": { "type": "integer", "description": "Starting line number, 1-indexed (default: 1)" },
                        "lines": { "type": "integer", "description": "Number of lines to read (default: 50, max: 200)" }
                    },
                    "additionalProperties": false
                }),
                strict: Some(true),
            },
        },
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "load_skill".to_string(),
                description: "Load a specific dynamic skill into your session context. This adds specialized instructions to your system prompt. You should ONLY load a skill if it is strictly necessary to answer the user's current prompt. You MUST unload the skill when the specific mini-task is complete to avoid context pollution. Please check the 'Available Skills' section in your system prompt to see what skills you can load.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "The exact name of the skill to load" },
                    },
                    "required": ["name"],
                    "additionalProperties": false
                }),
                strict: Some(true),
            },
        },
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "unload_skill".to_string(),
                description: "Unload a specific dynamic skill from your session context. You MUST do this immediately after you have finished the specific mini-task that required the skill.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "The name of the skill to unload" },
                    },
                    "required": ["name"],
                    "additionalProperties": false
                }),
                strict: Some(true),
            },
        },
        ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "list_skills".to_string(),
                description: "List all dynamically loadable skills available in the workspace. Use this to discover expertise you can adopt.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                }),
                strict: Some(true),
            },
        },
    ];

    all_tools
        .into_iter()
        .filter(|t| {
            global_tools.contains(&t.function.name.as_str())
                || required_tools.contains(&t.function.name)
        })
        .collect()
}
