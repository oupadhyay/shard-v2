//! Provider-neutral tool schema DTOs.
//!
//! Tool metadata is shared by the Tauri host, provider request builders, MCP,
//! and future standalone tool crates. Keep these types free of `Agent`, Tauri
//! state, lifecycle hooks, cache policy, and persisted chat-session concerns.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy)]
pub struct ToolInvocation<'a> {
    pub name: &'a str,
    pub args: &'a Value,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ToolDefinition {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: FunctionDefinition,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FunctionDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value,
    /// Required by Groq for proper tool calling.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tool_definition_serializes_openai_compatible_shape() {
        let definition = ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "lookup".to_string(),
                description: "Lookup things".to_string(),
                parameters: json!({"type": "object"}),
                strict: None,
            },
        };

        let serialized = serde_json::to_value(definition).unwrap();

        assert_eq!(serialized["type"], "function");
        assert!(serialized.get("tool_type").is_none());
        assert_eq!(serialized["function"]["name"], "lookup");
        assert!(serialized["function"].get("strict").is_none());
    }
}
