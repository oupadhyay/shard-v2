//! Phase 1 — pure-function tests.
//!
//! Covers every branch of the three pure helpers in `agent/mod.rs`:
//!   1. `normalize_gemini_schema` — JSON Schema sanitization for Gemini.
//!   2. `calculate_history_hash` — content-addressed history hashing.
//!   3. `split_transcript_chunks` — UTF-8-safe transcript chunking.
//!
//! These tests run without an `AppHandle`, network, SQLite, or filesystem.
//! They form the regression baseline for Phase 6's structural refactor.

#![cfg(test)]

use serde_json::json;

use crate::agent::{
    calculate_history_hash, normalize_gemini_schema, split_transcript_chunks, ChatMessage,
    FunctionCall, ToolCall,
};

// ============================================================================
// normalize_gemini_schema (12 branches)
// ============================================================================

mod normalize_schema {
    use super::*;

    /// Helper: round-trip a schema literal through the normalizer.
    fn norm(mut v: serde_json::Value) -> serde_json::Value {
        normalize_gemini_schema(&mut v);
        v
    }

    #[test]
    fn b1_scalar_type_preserved() {
        let out = norm(json!({"type": "string"}));
        assert_eq!(out["type"], "string");
    }

    #[test]
    fn b2_collapses_x_null_union_to_x() {
        let out = norm(json!({"type": ["string", "null"]}));
        assert_eq!(out["type"], "string");
    }

    #[test]
    fn b3_collapses_null_x_union_to_x() {
        // First non-null wins regardless of position.
        let out = norm(json!({"type": ["null", "integer"]}));
        assert_eq!(out["type"], "integer");
    }

    #[test]
    fn b4_all_null_array_falls_back_to_first() {
        let out = norm(json!({"type": ["null", "null"]}));
        assert_eq!(out["type"], "null");
    }

    #[test]
    fn b5_strips_additional_properties() {
        let out = norm(json!({"type": "object", "additionalProperties": false}));
        assert!(out
            .as_object()
            .unwrap()
            .get("additionalProperties")
            .is_none());
        assert_eq!(out["type"], "object");
    }

    #[test]
    fn b6_strips_strict_field() {
        let out = norm(json!({"type": "string", "strict": true}));
        assert!(out.as_object().unwrap().get("strict").is_none());
    }

    #[test]
    fn b7_recurses_into_properties_values() {
        let out = norm(json!({
            "type": "object",
            "properties": {
                "name": {"type": ["string", "null"]},
                "age": {"type": "integer", "additionalProperties": true}
            }
        }));
        assert_eq!(out["properties"]["name"]["type"], "string");
        assert_eq!(out["properties"]["age"]["type"], "integer");
        assert!(out["properties"]["age"]
            .as_object()
            .unwrap()
            .get("additionalProperties")
            .is_none());
    }

    #[test]
    fn b8_recurses_into_items() {
        let out = norm(json!({
            "type": "array",
            "items": {"type": ["number", "null"]}
        }));
        assert_eq!(out["items"]["type"], "number");
    }

    #[test]
    fn b9_recurses_into_additional_items_and_not() {
        let out = norm(json!({
            "additionalItems": {"type": ["boolean", "null"]},
            "not": {"type": ["string", "null"]}
        }));
        assert_eq!(out["additionalItems"]["type"], "boolean");
        assert_eq!(out["not"]["type"], "string");
    }

    #[test]
    fn b10_recurses_into_compositions() {
        let out = norm(json!({
            "allOf": [{"type": ["string", "null"]}],
            "anyOf": [{"type": ["integer", "null"]}],
            "oneOf": [{"type": ["number", "null"]}]
        }));
        assert_eq!(out["allOf"][0]["type"], "string");
        assert_eq!(out["anyOf"][0]["type"], "integer");
        assert_eq!(out["oneOf"][0]["type"], "number");
    }

    #[test]
    fn b11_non_object_schema_is_noop() {
        let out = norm(json!("just-a-string"));
        assert_eq!(out, json!("just-a-string"));
        let out = norm(json!(42));
        assert_eq!(out, json!(42));
        let out = norm(json!(null));
        assert_eq!(out, json!(null));
    }

    #[test]
    fn b12_deeply_nested_schema_normalized_throughout() {
        // End-to-end: a realistic tool param schema with multiple levels.
        let out = norm(json!({
            "type": "object",
            "additionalProperties": false,
            "strict": true,
            "properties": {
                "outer": {
                    "type": "object",
                    "properties": {
                        "inner": {
                            "type": "array",
                            "items": {
                                "type": ["string", "null"],
                                "additionalProperties": false
                            }
                        }
                    }
                }
            }
        }));
        // Top-level OpenAI-only fields are gone.
        let obj = out.as_object().unwrap();
        assert!(obj.get("additionalProperties").is_none());
        assert!(obj.get("strict").is_none());
        // Deeply nested array item type is collapsed and stripped.
        let item_schema = &out["properties"]["outer"]["properties"]["inner"]["items"];
        assert_eq!(item_schema["type"], "string");
        assert!(item_schema
            .as_object()
            .unwrap()
            .get("additionalProperties")
            .is_none());
    }
}

// ============================================================================
// calculate_history_hash (4 branches)
// ============================================================================

mod history_hash {
    use super::*;

    fn user(content: &str) -> ChatMessage {
        ChatMessage {
            role: "user".into(),
            content: Some(content.into()),
            reasoning: None,
            tool_calls: None,
            tool_call_id: None,
            is_cron: None,
            images: None,
        }
    }

    fn assistant_with_tool(name: &str, args: &str) -> ChatMessage {
        ChatMessage {
            role: "assistant".into(),
            content: None,
            reasoning: None,
            tool_calls: Some(vec![ToolCall {
                id: format!("call_{}", name),
                tool_type: "function".into(),
                function: FunctionCall {
                    name: name.into(),
                    arguments: args.into(),
                },
                thought_signature: None,
            }]),
            tool_call_id: None,
            is_cron: None,
            images: None,
        }
    }

    #[test]
    fn h1_empty_history_is_stable() {
        // Two empty calls must produce the same value (cannot panic, deterministic).
        assert_eq!(calculate_history_hash(&[]), calculate_history_hash(&[]));
    }

    #[test]
    fn h2_text_only_message_is_deterministic() {
        let h = vec![user("hello")];
        assert_eq!(calculate_history_hash(&h), calculate_history_hash(&h));
    }

    #[test]
    fn h3_changing_content_changes_hash() {
        let a = vec![user("hello")];
        let b = vec![user("world")];
        assert_ne!(calculate_history_hash(&a), calculate_history_hash(&b));
    }

    #[test]
    fn h4_tool_call_fields_participate_in_hash() {
        let no_tools = vec![ChatMessage {
            role: "assistant".into(),
            content: Some("ok".into()),
            reasoning: None,
            tool_calls: None,
            tool_call_id: None,
            is_cron: None,
            images: None,
        }];
        let with_tool = vec![assistant_with_tool("get_weather", "{\"loc\":\"Tokyo\"}")];
        let with_other_args = vec![assistant_with_tool("get_weather", "{\"loc\":\"NYC\"}")];

        let a = calculate_history_hash(&no_tools);
        let b = calculate_history_hash(&with_tool);
        let c = calculate_history_hash(&with_other_args);
        assert_ne!(a, b, "tool calls vs none must differ");
        assert_ne!(b, c, "different arguments must differ");
    }

    #[test]
    fn h5_reasoning_and_images_excluded_from_hash() {
        // Per the doc-comment contract: reasoning + images don't change the hash.
        let plain = vec![user("ping")];
        let with_reasoning = vec![ChatMessage {
            reasoning: Some("thinking deeply".into()),
            ..plain[0].clone()
        }];
        let with_images = vec![ChatMessage {
            images: Some(vec![crate::agent::ImageAttachment {
                base64: "abc".into(),
                mime_type: "image/png".into(),
                file_uri: None,
            }]),
            ..plain[0].clone()
        }];
        assert_eq!(
            calculate_history_hash(&plain),
            calculate_history_hash(&with_reasoning)
        );
        assert_eq!(
            calculate_history_hash(&plain),
            calculate_history_hash(&with_images)
        );
    }
}

// ============================================================================
// split_transcript_chunks (5 branches)
// ============================================================================

mod transcript_chunks {
    use super::*;

    #[test]
    fn j1_short_input_returns_single_chunk_equal_to_input() {
        let text = "short text";
        let chunks = split_transcript_chunks(text, 100);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], text);
    }

    #[test]
    fn j2_splits_on_newline_boundary_when_possible() {
        // 30 chars total, max=20 → first chunk should end at the newline before pos 20.
        let text = "line one\nline two\nline three end";
        let chunks = split_transcript_chunks(text, 20);
        // The first newline-aligned cut is right after "line one\n".
        assert!(chunks[0].ends_with('\n'), "first chunk: {:?}", chunks[0]);
        // Concatenation must be lossless.
        assert_eq!(chunks.concat(), text);
    }

    #[test]
    fn j3_no_newline_falls_back_to_char_boundary() {
        let text = "abcdefghijklmnopqrstuvwxyz"; // 26 chars, no newlines
        let chunks = split_transcript_chunks(text, 10);
        assert!(chunks.len() >= 3);
        // Concatenation lossless.
        assert_eq!(chunks.concat(), text);
        // No chunk exceeds max_chars.
        for c in &chunks {
            assert!(c.chars().count() <= 10);
        }
    }

    #[test]
    fn j4_multibyte_utf8_does_not_panic_or_split_mid_codepoint() {
        // Each emoji is 4 bytes but 1 char; ensure we slice by char.
        let text = "🦀🐙🐋🦊🦄🐉🦕🦖🐢🐍"; // 10 chars (10 emojis)
        let chunks = split_transcript_chunks(text, 3);
        // Lossless concatenation guarantees no mid-codepoint cut.
        assert_eq!(chunks.concat(), text);
        for c in &chunks {
            assert!(c.chars().count() <= 3);
            // Re-validating UTF-8 (str slice asserts this for us, but be explicit).
            assert!(std::str::from_utf8(c.as_bytes()).is_ok());
        }
    }

    #[test]
    fn j5_final_chunk_contains_remainder() {
        let text = "aaaaaaaaaa\nbbbbbbbbbb\nccc"; // 25 chars
        let chunks = split_transcript_chunks(text, 11);
        assert_eq!(chunks.concat(), text);
        // Last chunk holds the trailing "ccc" (possibly with prefix newline depending on split point).
        let last = chunks.last().unwrap();
        assert!(last.contains("ccc"));
    }
}
