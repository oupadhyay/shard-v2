/// Steps Schema Migration Tests (Api-Revision: 2026-05-20)
///
/// Validates that our SSE parser and event processor can handle the new
/// Interactions API steps schema that becomes default on May 26, 2026.
/// These tests use the exact JSON shapes from the official breaking changes
/// guide at https://ai.google.dev/gemini-api/docs/interactions-breaking-changes-may-2026
///
/// Once the migration code lands, these tests should pass. Until then, they
/// document the target contract and will fail against the current legacy parser.
#[cfg(test)]
mod tests {
    use crate::agent::{
        parse_interactions_sse_line, process_interactions_event,
        AgentEvent, InteractionDelta, InteractionStreamEvent,
    };
    use serde_json::{json, Value};

    // ========================================================================
    // 1. SSE Line Parsing — New Event Names
    //    Legacy: content.delta / content.start / content.stop / interaction.complete
    //    New:    step.delta   / step.start   / step.stop   / interaction.completed
    // ========================================================================

    #[test]
    fn test_parse_step_delta_text() {
        let sse = r#"data: {"event_type":"step.delta","index":1,"delta":{"type":"text","text":"Hello"}}"#;
        let event = parse_interactions_sse_line(sse).expect("should parse step.delta text");
        assert_eq!(event.event_type, "step.delta");
        assert_eq!(event.index, Some(1));
        if let Some(InteractionDelta::Text { text }) = &event.delta {
            assert_eq!(text, "Hello");
        } else {
            panic!("Expected Text delta, got {:?}", event.delta);
        }
    }

    #[test]
    fn test_parse_step_delta_thought() {
        let sse = r#"data: {"event_type":"step.delta","index":0,"delta":{"type":"thought","thought":"Analyzing the question..."}}"#;
        let event = parse_interactions_sse_line(sse).expect("should parse step.delta thought");
        assert_eq!(event.event_type, "step.delta");
        if let Some(InteractionDelta::Thought { thought }) = &event.delta {
            assert_eq!(thought.as_deref(), Some("Analyzing the question..."));
        } else {
            panic!("Expected Thought delta, got {:?}", event.delta);
        }
    }

    #[test]
    fn test_parse_step_delta_thought_summary() {
        let sse = r#"data: {"event_type":"step.delta","index":0,"delta":{"type":"thought_summary","content":{"text":"User wants weather info."}}}"#;
        let event = parse_interactions_sse_line(sse).expect("should parse thought_summary");
        assert_eq!(event.event_type, "step.delta");
        if let Some(InteractionDelta::ThoughtSummary { content }) = &event.delta {
            assert_eq!(content.as_ref().unwrap().text.as_deref(), Some("User wants weather info."));
        } else {
            panic!("Expected ThoughtSummary delta, got {:?}", event.delta);
        }
    }

    #[test]
    fn test_parse_step_delta_thought_signature() {
        let sse = r#"data: {"event_type":"step.delta","index":0,"delta":{"type":"thought_signature","signature":"sig_abc123"}}"#;
        let event = parse_interactions_sse_line(sse).expect("should parse thought_signature");
        assert_eq!(event.event_type, "step.delta");
        if let Some(InteractionDelta::ThoughtSignature { signature }) = &event.delta {
            assert_eq!(signature, "sig_abc123");
        } else {
            panic!("Expected ThoughtSignature delta, got {:?}", event.delta);
        }
    }

    #[test]
    fn test_parse_step_start_model_output() {
        // step.start for model_output carries initial content in the step payload
        let sse = r#"data: {"event_type":"step.start","index":1,"step":{"content":[{"text":"Once upon","type":"text"}],"type":"model_output"}}"#;
        let event = parse_interactions_sse_line(sse).expect("should parse step.start");
        assert_eq!(event.event_type, "step.start");
        assert_eq!(event.index, Some(1));
    }

    #[test]
    fn test_parse_step_start_thought() {
        // Thought steps arrive with signature in step.start, no deltas
        let sse = r#"data: {"event_type":"step.start","index":0,"step":{"type":"thought","signature":"abc123..."}}"#;
        let event = parse_interactions_sse_line(sse).expect("should parse thought step.start");
        assert_eq!(event.event_type, "step.start");
        assert_eq!(event.index, Some(0));
    }

    #[test]
    fn test_parse_step_stop() {
        let sse = r#"data: {"type":"step.stop","event_type":"step.stop","index":1,"status":"done"}"#;
        let event = parse_interactions_sse_line(sse).expect("should parse step.stop");
        assert_eq!(event.event_type, "step.stop");
        assert_eq!(event.index, Some(1));
    }

    #[test]
    fn test_parse_interaction_created() {
        let sse = r#"data: {"event_type":"interaction.created","interaction":{"id":"int_xyz","status":"in_progress","object":"interaction","model":"gemini-3-flash-preview"}}"#;
        let event = parse_interactions_sse_line(sse).expect("should parse interaction.created");
        assert_eq!(event.event_type, "interaction.created");
        let interaction = event.interaction.expect("should have interaction");
        assert_eq!(interaction.id.as_deref(), Some("int_xyz"));
    }

    #[test]
    fn test_parse_interaction_completed() {
        // New name: "interaction.completed" (was "interaction.complete")
        let sse = r#"data: {"type":"interaction.completed","event_type":"interaction.completed","interaction":{"id":"int_xyz","status":"completed","usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}}"#;
        let event = parse_interactions_sse_line(sse).expect("should parse interaction.completed");
        assert_eq!(event.event_type, "interaction.completed");
        let interaction = event.interaction.expect("should have interaction");
        assert_eq!(interaction.status.as_deref(), Some("completed"));
    }

    #[test]
    fn test_parse_interaction_in_progress() {
        let sse = r#"data: {"event_type":"interaction.in_progress","interaction_id":"int_xyz"}"#;
        let event = parse_interactions_sse_line(sse).expect("should parse interaction.in_progress");
        assert_eq!(event.event_type, "interaction.in_progress");
    }

    #[test]
    fn test_parse_interaction_requires_action() {
        let sse = r#"data: {"type":"interaction.requires_action","event_type":"interaction.requires_action","interaction":{"id":"int_xyz","status":"requires_action"}}"#;
        let event = parse_interactions_sse_line(sse).expect("should parse requires_action");
        assert_eq!(event.event_type, "interaction.requires_action");
    }

    // ========================================================================
    // 2. Streaming Function Call — New step-based lifecycle
    //    Legacy: single content.delta with complete function_call
    //    New:    step.start (name+id) → step.delta (arguments) → step.stop
    // ========================================================================

    #[test]
    fn test_parse_step_start_function_call() {
        // step.start for function_call delivers name + id, no arguments yet
        let sse = r#"data: {"type":"step.start","event_type":"step.start","index":1,"step":{"type":"function_call","id":"fc_1","name":"get_weather"}}"#;
        let event = parse_interactions_sse_line(sse).expect("should parse function_call step.start");
        assert_eq!(event.event_type, "step.start");
        assert_eq!(event.index, Some(1));
    }

    #[test]
    fn test_parse_step_delta_function_call_complete() {
        // When streamFunctionCallArguments is NOT enabled, args arrive complete
        // in a single step.delta with type "function_call"
        let sse = r#"data: {"event_type":"step.delta","index":1,"delta":{"type":"function_call","id":"fc_1","name":"get_weather","arguments":{"location":"Boston, MA"}}}"#;
        let event = parse_interactions_sse_line(sse).expect("should parse complete function_call delta");
        assert_eq!(event.event_type, "step.delta");
        if let Some(InteractionDelta::FunctionCallDelta { id, name, arguments }) = &event.delta {
            assert_eq!(id, "fc_1");
            assert_eq!(name, "get_weather");
            assert_eq!(arguments["location"], "Boston, MA");
        } else {
            panic!("Expected FunctionCallDelta, got {:?}", event.delta);
        }
    }

    #[test]
    fn test_parse_step_stop_function_call_waiting() {
        // Function call step.stop has status "waiting" (requires tool result)
        let sse = r#"data: {"type":"step.stop","event_type":"step.stop","index":1,"status":"waiting"}"#;
        let event = parse_interactions_sse_line(sse).expect("should parse waiting step.stop");
        assert_eq!(event.event_type, "step.stop");
    }

    // ========================================================================
    // 3. process_interactions_event — step.delta handling
    //    Verify that the event processor correctly handles the new event name
    //    while the delta payload structure remains identical for text/thought.
    // ========================================================================

    #[test]
    fn test_process_step_delta_text() {
        let event = InteractionStreamEvent {
            event_type: "step.delta".to_string(),
            index: Some(1),
            delta: Some(InteractionDelta::Text {
                text: "Hello world".to_string(),
            }),
            content: None,
            interaction: None,
        };

        let mut full_text = String::new();
        let mut full_reasoning = String::new();
        let events = process_interactions_event(&event, &mut full_text, &mut full_reasoning);

        assert_eq!(events.len(), 1);
        assert_eq!(full_text, "Hello world");
        assert!(full_reasoning.is_empty());
        assert!(matches!(&events[0], AgentEvent::ResponseChunk(t) if t == "Hello world"));
    }

    #[test]
    fn test_process_step_delta_thought() {
        let event = InteractionStreamEvent {
            event_type: "step.delta".to_string(),
            index: Some(0),
            delta: Some(InteractionDelta::Thought {
                thought: Some("Let me think...".to_string()),
            }),
            content: None,
            interaction: None,
        };

        let mut full_text = String::new();
        let mut full_reasoning = String::new();
        let events = process_interactions_event(&event, &mut full_text, &mut full_reasoning);

        assert_eq!(events.len(), 1);
        assert!(full_text.is_empty());
        assert_eq!(full_reasoning, "Let me think...");
        assert!(matches!(&events[0], AgentEvent::ReasoningChunk(t) if t == "Let me think..."));
    }

    #[test]
    fn test_process_step_delta_function_call() {
        let event = InteractionStreamEvent {
            event_type: "step.delta".to_string(),
            index: Some(1),
            delta: Some(InteractionDelta::FunctionCallDelta {
                id: "fc_abc".to_string(),
                name: "search_wikipedia".to_string(),
                arguments: json!({"query": "Rust programming"}),
            }),
            content: None,
            interaction: None,
        };

        let mut full_text = String::new();
        let mut full_reasoning = String::new();
        let events = process_interactions_event(&event, &mut full_text, &mut full_reasoning);

        assert_eq!(events.len(), 1);
        if let AgentEvent::InteractionToolCall { id, name, arguments, signature } = &events[0] {
            assert_eq!(id, "fc_abc");
            assert_eq!(name, "search_wikipedia");
            assert_eq!(arguments["query"], "Rust programming");
            assert!(signature.is_none());
        } else {
            panic!("Expected InteractionToolCall event");
        }
    }

    #[test]
    fn test_process_step_delta_thought_signature() {
        let event = InteractionStreamEvent {
            event_type: "step.delta".to_string(),
            index: Some(0),
            delta: Some(InteractionDelta::ThoughtSignature {
                signature: "sig_xyz".to_string(),
            }),
            content: None,
            interaction: None,
        };

        let mut full_text = String::new();
        let mut full_reasoning = String::new();
        let events = process_interactions_event(&event, &mut full_text, &mut full_reasoning);

        assert_eq!(events.len(), 1);
        if let AgentEvent::InteractionToolCall { signature, .. } = &events[0] {
            assert_eq!(signature.as_deref(), Some("sig_xyz"));
        } else {
            panic!("Expected InteractionToolCall with signature");
        }
    }

    #[test]
    fn test_process_step_delta_accumulates_text() {
        let mut full_text = String::new();
        let mut full_reasoning = String::new();

        let e1 = InteractionStreamEvent {
            event_type: "step.delta".to_string(),
            index: Some(1),
            delta: Some(InteractionDelta::Text { text: "Hello ".to_string() }),
            content: None,
            interaction: None,
        };
        process_interactions_event(&e1, &mut full_text, &mut full_reasoning);

        let e2 = InteractionStreamEvent {
            event_type: "step.delta".to_string(),
            index: Some(1),
            delta: Some(InteractionDelta::Text { text: "world!".to_string() }),
            content: None,
            interaction: None,
        };
        process_interactions_event(&e2, &mut full_text, &mut full_reasoning);

        assert_eq!(full_text, "Hello world!");
    }

    #[test]
    fn test_process_step_delta_mixed_thought_and_text() {
        let mut full_text = String::new();
        let mut full_reasoning = String::new();

        // Thought first
        let thought_event = InteractionStreamEvent {
            event_type: "step.delta".to_string(),
            index: Some(0),
            delta: Some(InteractionDelta::Thought {
                thought: Some("Planning response.".to_string()),
            }),
            content: None,
            interaction: None,
        };
        let r = process_interactions_event(&thought_event, &mut full_text, &mut full_reasoning);
        assert_eq!(r.len(), 1);

        // Then text
        let text_event = InteractionStreamEvent {
            event_type: "step.delta".to_string(),
            index: Some(1),
            delta: Some(InteractionDelta::Text {
                text: "Here is the answer.".to_string(),
            }),
            content: None,
            interaction: None,
        };
        let r = process_interactions_event(&text_event, &mut full_text, &mut full_reasoning);
        assert_eq!(r.len(), 1);

        assert_eq!(full_reasoning, "Planning response.");
        assert_eq!(full_text, "Here is the answer.");
    }

    #[test]
    fn test_process_unknown_event_type_produces_no_events() {
        let event = InteractionStreamEvent {
            event_type: "interaction.created".to_string(),
            index: None,
            delta: None,
            content: None,
            interaction: None,
        };

        let mut ft = String::new();
        let mut fr = String::new();
        let events = process_interactions_event(&event, &mut ft, &mut fr);
        assert!(events.is_empty());
    }

    // ========================================================================
    // 4. Unary Response — Steps Schema Deserialization
    //    Validates that the new JSON shape with `steps` array can be parsed.
    // ========================================================================

    #[test]
    fn test_deserialize_steps_response_basic_text() {
        let body: Value = json!({
            "id": "int_123",
            "steps": [
                {
                    "type": "model_output",
                    "status": "done",
                    "content": [
                        { "type": "text", "text": "Why did the chicken cross the road?" }
                    ]
                }
            ]
        });

        let steps = body["steps"].as_array().expect("steps should be array");
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0]["type"], "model_output");
        let content = steps[0]["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "Why did the chicken cross the road?");
    }

    #[test]
    fn test_deserialize_steps_response_with_thought() {
        let body: Value = json!({
            "id": "int_001",
            "status": "requires_action",
            "steps": [
                {
                    "type": "thought",
                    "summary": [{ "type": "text", "text": "I need to check the weather in Boston..." }],
                    "signature": "abc123..."
                },
                {
                    "type": "function_call",
                    "id": "fc_1",
                    "name": "get_weather",
                    "arguments": { "location": "Boston, MA" }
                }
            ]
        });

        let steps = body["steps"].as_array().unwrap();
        assert_eq!(steps.len(), 2);

        // Thought step — summary is now an array, not a flat string
        assert_eq!(steps[0]["type"], "thought");
        let summary = steps[0]["summary"].as_array().expect("summary should be array");
        assert_eq!(summary[0]["text"], "I need to check the weather in Boston...");
        assert_eq!(steps[0]["signature"], "abc123...");

        // Function call step
        assert_eq!(steps[1]["type"], "function_call");
        assert_eq!(steps[1]["id"], "fc_1");
        assert_eq!(steps[1]["name"], "get_weather");
        assert_eq!(steps[1]["arguments"]["location"], "Boston, MA");
    }

    #[test]
    fn test_deserialize_steps_response_function_result_then_output() {
        // After submitting a tool result, the response contains the echoed
        // function_result step followed by the final model_output
        let body: Value = json!({
            "id": "int_002",
            "status": "completed",
            "steps": [
                {
                    "type": "function_result",
                    "call_id": "fc_1",
                    "name": "get_weather",
                    "result": [{ "type": "text", "text": "52°F with rain" }]
                },
                {
                    "type": "model_output",
                    "status": "done",
                    "content": [
                        { "type": "text", "text": "It's 52°F with rain in Boston." }
                    ]
                }
            ]
        });

        let steps = body["steps"].as_array().unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0]["type"], "function_result");
        assert_eq!(steps[0]["call_id"], "fc_1");
        assert_eq!(steps[1]["type"], "model_output");
        assert_eq!(steps[1]["content"][0]["text"], "It's 52°F with rain in Boston.");
    }

    #[test]
    fn test_deserialize_steps_response_google_search() {
        let body: Value = json!({
            "id": "int_456",
            "steps": [
                {
                    "type": "google_search_call",
                    "id": "gs_1",
                    "arguments": { "queries": ["last Super Bowl winner"] },
                    "signature": "abc123..."
                },
                {
                    "type": "google_search_result",
                    "call_id": "gs_1",
                    "result": { "search_suggestions": "<div>...</div>" },
                    "signature": "abc123..."
                },
                {
                    "type": "model_output",
                    "content": [{
                        "type": "text",
                        "text": "The Kansas City Chiefs won.",
                        "annotations": [{
                            "type": "url_citation",
                            "url": "https://www.nfl.com/super-bowl",
                            "title": "NFL.com",
                            "start_index": 4,
                            "end_index": 26
                        }]
                    }]
                }
            ],
            "status": "completed"
        });

        let steps = body["steps"].as_array().unwrap();
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[0]["type"], "google_search_call");
        assert_eq!(steps[1]["type"], "google_search_result");
        assert_eq!(steps[2]["type"], "model_output");
        // Annotations are now inline in the text content item
        let annotations = steps[2]["content"][0]["annotations"].as_array().unwrap();
        assert_eq!(annotations[0]["type"], "url_citation");
    }

    // ========================================================================
    // 5. Extract model text from steps — helper validation
    //    This is the core logic that background.rs call_gemini_oneshot needs:
    //    steps → find model_output → content[0].text
    // ========================================================================

    /// Reference implementation of the extraction logic that will be needed
    /// in background.rs to replace `body.get("outputs")` traversal.
    fn extract_model_text_from_steps(body: &Value) -> Option<String> {
        let steps = body.get("steps")?.as_array()?;
        for step in steps {
            let step_type = step.get("type")?.as_str()?;
            if step_type == "model_output" {
                let content = step.get("content")?.as_array()?;
                for item in content {
                    if item.get("type").and_then(|t| t.as_str()) == Some("text") {
                        return item.get("text").and_then(|t| t.as_str()).map(|s| s.to_string());
                    }
                }
            }
        }
        None
    }

    #[test]
    fn test_extract_model_text_simple() {
        let body = json!({
            "id": "int_1",
            "steps": [{
                "type": "model_output",
                "status": "done",
                "content": [{ "type": "text", "text": "The answer is 42." }]
            }]
        });
        assert_eq!(extract_model_text_from_steps(&body), Some("The answer is 42.".to_string()));
    }

    #[test]
    fn test_extract_model_text_with_preceding_thought() {
        let body = json!({
            "id": "int_2",
            "steps": [
                { "type": "thought", "signature": "sig1" },
                { "type": "model_output", "content": [{ "type": "text", "text": "Final answer." }] }
            ]
        });
        assert_eq!(extract_model_text_from_steps(&body), Some("Final answer.".to_string()));
    }

    #[test]
    fn test_extract_model_text_returns_none_for_no_model_output() {
        let body = json!({
            "id": "int_3",
            "status": "requires_action",
            "steps": [
                { "type": "function_call", "id": "fc_1", "name": "get_weather", "arguments": {} }
            ]
        });
        assert_eq!(extract_model_text_from_steps(&body), None);
    }

    #[test]
    fn test_extract_model_text_returns_none_for_empty_steps() {
        let body = json!({ "id": "int_4", "steps": [] });
        assert_eq!(extract_model_text_from_steps(&body), None);
    }

    #[test]
    fn test_extract_model_text_returns_none_for_missing_steps() {
        // Legacy response with outputs — should return None from steps extractor
        let body = json!({
            "id": "int_5",
            "outputs": [{ "type": "text", "text": "legacy response" }]
        });
        assert_eq!(extract_model_text_from_steps(&body), None);
    }

    // ========================================================================
    // 6. Full streaming session simulation
    //    Replays a complete step-based SSE stream to verify end-to-end parsing.
    // ========================================================================

    #[test]
    fn test_full_streaming_session_text_only() {
        let sse_lines = vec![
            r#"data: {"event_type":"interaction.created","interaction":{"id":"int_xyz","status":"in_progress"}}"#,
            r#"data: {"event_type":"interaction.in_progress","interaction_id":"int_xyz"}"#,
            r#"data: {"event_type":"step.start","index":0,"step":{"type":"thought","signature":"abc123"}}"#,
            r#"data: {"event_type":"step.delta","index":0,"delta":{"type":"thought","thought":"Let me think."}}"#,
            r#"data: {"event_type":"step.stop","index":0}"#,
            r#"data: {"event_type":"step.start","index":1,"step":{"type":"model_output","content":[{"text":"Once upon","type":"text"}]}}"#,
            r#"data: {"event_type":"step.delta","index":1,"delta":{"type":"text","text":" a time..."}}"#,
            r#"data: {"event_type":"step.stop","index":1,"status":"done"}"#,
            r#"data: {"event_type":"interaction.completed","interaction":{"id":"int_xyz","status":"completed","usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}}"#,
        ];

        let mut full_text = String::new();
        let mut full_reasoning = String::new();
        let mut all_events: Vec<AgentEvent> = Vec::new();

        for line in &sse_lines {
            if let Some(event) = parse_interactions_sse_line(line) {
                let evts = process_interactions_event(&event, &mut full_text, &mut full_reasoning);
                all_events.extend(evts);
            }
        }

        assert_eq!(full_reasoning, "Let me think.");
        assert_eq!(full_text, " a time...");
        // Should have: 1 reasoning chunk + 1 text chunk = 2 events
        assert_eq!(all_events.len(), 2);
    }

    #[test]
    fn test_full_streaming_session_with_tool_call() {
        let sse_lines = vec![
            r#"data: {"event_type":"interaction.created","interaction":{"id":"int_xyz","status":"in_progress"}}"#,
            r#"data: {"event_type":"step.start","index":0,"step":{"type":"thought"}}"#,
            r#"data: {"event_type":"step.delta","index":0,"delta":{"type":"thought","thought":"I'll call get_weather."}}"#,
            r#"data: {"event_type":"step.stop","index":0,"status":"done"}"#,
            // Function call step — args arrive complete in one delta
            r#"data: {"event_type":"step.start","index":1,"step":{"type":"function_call","id":"fc_1","name":"get_weather"}}"#,
            r#"data: {"event_type":"step.delta","index":1,"delta":{"type":"function_call","id":"fc_1","name":"get_weather","arguments":{"location":"Boston, MA"}}}"#,
            r#"data: {"event_type":"step.stop","index":1,"status":"waiting"}"#,
            r#"data: {"event_type":"interaction.requires_action","interaction":{"id":"int_xyz","status":"requires_action"}}"#,
        ];

        let mut full_text = String::new();
        let mut full_reasoning = String::new();
        let mut all_events: Vec<AgentEvent> = Vec::new();

        for line in &sse_lines {
            if let Some(event) = parse_interactions_sse_line(line) {
                let evts = process_interactions_event(&event, &mut full_text, &mut full_reasoning);
                all_events.extend(evts);
            }
        }

        assert_eq!(full_reasoning, "I'll call get_weather.");
        assert!(full_text.is_empty()); // No model output yet — waiting for tool result

        // Should have: 1 reasoning + 1 tool call = 2 events
        assert_eq!(all_events.len(), 2);
        let tool_event = &all_events[1];
        if let AgentEvent::InteractionToolCall { id, name, arguments, .. } = tool_event {
            assert_eq!(id, "fc_1");
            assert_eq!(name, "get_weather");
            assert_eq!(arguments["location"], "Boston, MA");
        } else {
            panic!("Expected InteractionToolCall event");
        }
    }
}
