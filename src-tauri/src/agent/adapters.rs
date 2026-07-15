use super::types::{ChatMessage, ToolDefinition};
use crate::llm_provider::{
    ProviderFunctionCall, ProviderFunctionDefinition, ProviderImage, ProviderMessage,
    ProviderToolCall, ProviderToolDefinition,
};

pub(crate) fn chat_message_to_provider(message: &ChatMessage) -> ProviderMessage {
    ProviderMessage {
        role: message.role.clone(),
        content: message.content.clone(),
        reasoning: message.reasoning.clone(),
        tool_calls: message.tool_calls.as_ref().map(|calls| {
            calls
                .iter()
                .map(|call| ProviderToolCall {
                    id: call.id.clone(),
                    tool_type: call.tool_type.clone(),
                    function: ProviderFunctionCall {
                        name: call.function.name.clone(),
                        arguments: call.function.arguments.clone(),
                    },
                    thought_signature: call.thought_signature.clone(),
                })
                .collect()
        }),
        tool_call_id: message.tool_call_id.clone(),
        images: message.images.as_ref().map(|images| {
            images
                .iter()
                .map(|image| ProviderImage {
                    base64: image.base64.clone(),
                    mime_type: image.mime_type.clone(),
                    file_uri: image.file_uri.clone(),
                })
                .collect()
        }),
    }
}

pub(crate) fn chat_messages_to_provider(messages: &[ChatMessage]) -> Vec<ProviderMessage> {
    messages.iter().map(chat_message_to_provider).collect()
}

pub(crate) fn provider_tool_definition_from_host(
    definition: &ToolDefinition,
) -> ProviderToolDefinition {
    ProviderToolDefinition {
        tool_type: definition.tool_type.clone(),
        function: ProviderFunctionDefinition {
            name: definition.function.name.clone(),
            description: definition.function.description.clone(),
            parameters: definition.function.parameters.clone(),
            strict: definition.function.strict,
        },
    }
}

pub(crate) fn provider_tool_call_to_host(call: &ProviderToolCall) -> super::types::ToolCall {
    super::types::ToolCall {
        id: call.id.clone(),
        tool_type: call.tool_type.clone(),
        function: super::types::FunctionCall {
            name: call.function.name.clone(),
            arguments: call.function.arguments.clone(),
        },
        thought_signature: call.thought_signature.clone(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::agent::types::{
        FunctionCall, FunctionDefinition, ImageAttachment, ToolCall, ToolDefinition,
    };

    #[test]
    fn chat_message_to_provider_preserves_boundary_fields() {
        let message = ChatMessage {
            role: "assistant".to_string(),
            content: Some("Checking".to_string()),
            reasoning: Some("thinking".to_string()),
            tool_calls: Some(vec![ToolCall {
                id: "call_1".to_string(),
                tool_type: "function".to_string(),
                function: FunctionCall {
                    name: "search".to_string(),
                    arguments: "{\"q\":\"rust\"}".to_string(),
                },
                thought_signature: Some("sig".to_string()),
            }]),
            tool_call_id: Some("parent_call".to_string()),
            is_cron: Some(true),
            images: Some(vec![ImageAttachment {
                base64: "img".to_string(),
                mime_type: "image/png".to_string(),
                file_uri: Some("file://gemini".to_string()),
            }]),
        };

        let provider = chat_message_to_provider(&message);

        assert_eq!(provider.role, "assistant");
        assert_eq!(provider.content.as_deref(), Some("Checking"));
        assert_eq!(provider.reasoning.as_deref(), Some("thinking"));
        assert_eq!(provider.tool_call_id.as_deref(), Some("parent_call"));
        assert_eq!(provider.tool_calls.as_ref().unwrap()[0].id, "call_1");
        assert_eq!(
            provider.tool_calls.as_ref().unwrap()[0]
                .thought_signature
                .as_deref(),
            Some("sig")
        );
        assert_eq!(provider.images.as_ref().unwrap()[0].mime_type, "image/png");
    }

    #[test]
    fn provider_tool_definition_from_host_preserves_schema_and_strict() {
        let definition = ToolDefinition {
            tool_type: "function".to_string(),
            function: FunctionDefinition {
                name: "lookup".to_string(),
                description: "Lookup things".to_string(),
                parameters: json!({"type": "object"}),
                strict: Some(true),
            },
        };

        let provider = provider_tool_definition_from_host(&definition);

        assert_eq!(provider.tool_type, "function");
        assert_eq!(provider.function.name, "lookup");
        assert_eq!(provider.function.parameters, json!({"type": "object"}));
        assert_eq!(provider.function.strict, Some(true));
    }

    #[test]
    fn provider_tool_call_to_host_preserves_call_fields() {
        let provider = ProviderToolCall {
            id: "call_2".to_string(),
            tool_type: "function".to_string(),
            function: ProviderFunctionCall {
                name: "write".to_string(),
                arguments: "{\"path\":\"x\"}".to_string(),
            },
            thought_signature: Some("sig2".to_string()),
        };

        let host = provider_tool_call_to_host(&provider);

        assert_eq!(host.id, "call_2");
        assert_eq!(host.tool_type, "function");
        assert_eq!(host.function.name, "write");
        assert_eq!(host.function.arguments, "{\"path\":\"x\"}");
        assert_eq!(host.thought_signature.as_deref(), Some("sig2"));
    }
}
