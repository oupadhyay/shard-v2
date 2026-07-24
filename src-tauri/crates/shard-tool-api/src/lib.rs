//! Provider-neutral tool schema DTOs.
//!
//! Tool metadata is shared by the Shard host, provider request builders, MCP,
//! and standalone tool crates. These types intentionally contain no host state,
//! lifecycle policy, cache behavior, or persisted chat-session concerns.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy)]
pub struct ToolInvocation<'a> {
    pub name: &'a str,
    pub args: &'a Value,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ToolDefinition {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: FunctionDefinition,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
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
    fn tool_definition_round_trips_openai_compatible_shape() {
        let definition = ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "lookup".to_string(),
                description: "Lookup things".to_string(),
                parameters: json!({"type": "object"}),
                strict: None,
            },
        };

        let serialized = serde_json::to_value(&definition).unwrap();

        assert_eq!(
            serialized,
            json!({
                "type": "function",
                "function": {
                    "name": "lookup",
                    "description": "Lookup things",
                    "parameters": {"type": "object"}
                }
            })
        );
        assert_eq!(
            serde_json::from_value::<ToolDefinition>(serialized).unwrap(),
            definition
        );
    }

    #[test]
    fn strict_serializes_when_present() {
        let definition = ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "lookup".to_string(),
                description: "Lookup things".to_string(),
                parameters: json!({"type": "object"}),
                strict: Some(true),
            },
        };

        assert_eq!(
            serde_json::to_value(definition).unwrap()["function"]["strict"],
            true
        );
    }
}
