/// Steps Schema Migration Tests (Api-Revision: 2026-05-20)
///
/// Validates that the SSE parser and event processor correctly handle the new
/// Interactions API steps schema that becomes default on May 26, 2026.
/// Fixtures use the exact JSON shapes from the official breaking changes guide.
#[cfg(test)]
mod tests {
    use crate::agent::{
        extract_model_text_from_steps, parse_interactions_sse_line, process_interactions_event,
        AgentEvent, InteractionDelta, InteractionStreamEvent, GEMINI_API_REVISION,
    };
    use serde_json::{json, Value};

    // ========================================================================
    // 0. Shared constants
    // ========================================================================

    #[test]
    fn test_api_revision_value() {
        assert_eq!(GEMINI_API_REVISION, "2026-05-20");
    }

    // ========================================================================
    // 1. SSE Line Parsing — New Event Names
    //    Legacy: content.delta / content.start / content.stop / interaction.complete
    //    New:    step.delta   / step.start   / step.stop   / interaction.completed
    // ========================================================================

    #[test]
    fn test_parse_step_delta_text() {
        let sse =
            r#"data: {"event_type":"step.delta","index":1,"delta":{"type":"text","text":"Hello"}}"#;
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
            assert_eq!(
                content.as_ref().unwrap().text.as_deref(),
                Some("User wants weather info.")
            );
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
        let sse = r#"data: {"event_type":"step.start","index":1,"step":{"content":[{"text":"Once upon","type":"text"}],"type":"model_output"}}"#;
        let event = parse_interactions_sse_line(sse).expect("should parse step.start");
        assert_eq!(event.event_type, "step.start");
        assert_eq!(event.index, Some(1));
        // Verify the step payload is accessible
        let step = event.step.expect("step.start should have step payload");
        assert_eq!(step["type"], "model_output");
        assert_eq!(step["content"][0]["text"], "Once upon");
    }

    #[test]
    fn test_parse_step_start_thought() {
        let sse = r#"data: {"event_type":"step.start","index":0,"step":{"type":"thought","signature":"abc123..."}}"#;
        let event = parse_interactions_sse_line(sse).expect("should parse thought step.start");
        assert_eq!(event.event_type, "step.start");
        let step = event.step.expect("should have step payload");
        assert_eq!(step["type"], "thought");
        assert_eq!(step["signature"], "abc123...");
    }

    #[test]
    fn test_parse_step_stop() {
        let sse =
            r#"data: {"type":"step.stop","event_type":"step.stop","index":1,"status":"done"}"#;
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
    // ========================================================================

    #[test]
    fn test_parse_step_start_function_call() {
        let sse = r#"data: {"type":"step.start","event_type":"step.start","index":1,"step":{"type":"function_call","id":"fc_1","name":"get_weather"}}"#;
        let event =
            parse_interactions_sse_line(sse).expect("should parse function_call step.start");
        assert_eq!(event.event_type, "step.start");
        let step = event.step.expect("should have step payload");
        assert_eq!(step["type"], "function_call");
        assert_eq!(step["name"], "get_weather");
        assert_eq!(step["id"], "fc_1");
    }

    #[test]
    fn test_parse_step_delta_function_call_complete() {
        let sse = r#"data: {"event_type":"step.delta","index":1,"delta":{"type":"function_call","id":"fc_1","name":"get_weather","arguments":{"location":"Boston, MA"}}}"#;
        let event =
            parse_interactions_sse_line(sse).expect("should parse complete function_call delta");
        assert_eq!(event.event_type, "step.delta");
        if let Some(InteractionDelta::FunctionCallDelta {
            id,
            name,
            arguments,
        }) = &event.delta
        {
            assert_eq!(id, "fc_1");
            assert_eq!(name, "get_weather");
            assert_eq!(arguments["location"], "Boston, MA");
        } else {
            panic!("Expected FunctionCallDelta, got {:?}", event.delta);
        }
    }

    #[test]
    fn test_parse_step_stop_function_call_waiting() {
        let sse =
            r#"data: {"type":"step.stop","event_type":"step.stop","index":1,"status":"waiting"}"#;
        let event = parse_interactions_sse_line(sse).expect("should parse waiting step.stop");
        assert_eq!(event.event_type, "step.stop");
    }

    // ========================================================================
    // 3. process_interactions_event — step.delta handling
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
            step: None,
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
            step: None,
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
            step: None,
        };

        let mut full_text = String::new();
        let mut full_reasoning = String::new();
        let events = process_interactions_event(&event, &mut full_text, &mut full_reasoning);

        assert_eq!(events.len(), 1);
        if let AgentEvent::InteractionToolCall {
            id,
            name,
            arguments,
            signature,
        } = &events[0]
        {
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
            step: None,
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
            delta: Some(InteractionDelta::Text {
                text: "Hello ".to_string(),
            }),
            content: None,
            interaction: None,
            step: None,
        };
        process_interactions_event(&e1, &mut full_text, &mut full_reasoning);

        let e2 = InteractionStreamEvent {
            event_type: "step.delta".to_string(),
            index: Some(1),
            delta: Some(InteractionDelta::Text {
                text: "world!".to_string(),
            }),
            content: None,
            interaction: None,
            step: None,
        };
        process_interactions_event(&e2, &mut full_text, &mut full_reasoning);

        assert_eq!(full_text, "Hello world!");
    }

    #[test]
    fn test_process_step_delta_mixed_thought_and_text() {
        let mut full_text = String::new();
        let mut full_reasoning = String::new();

        let thought_event = InteractionStreamEvent {
            event_type: "step.delta".to_string(),
            index: Some(0),
            delta: Some(InteractionDelta::Thought {
                thought: Some("Planning response.".to_string()),
            }),
            content: None,
            interaction: None,
            step: None,
        };
        let r = process_interactions_event(&thought_event, &mut full_text, &mut full_reasoning);
        assert_eq!(r.len(), 1);

        let text_event = InteractionStreamEvent {
            event_type: "step.delta".to_string(),
            index: Some(1),
            delta: Some(InteractionDelta::Text {
                text: "Here is the answer.".to_string(),
            }),
            content: None,
            interaction: None,
            step: None,
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
            step: None,
        };
        let mut ft = String::new();
        let mut fr = String::new();
        let events = process_interactions_event(&event, &mut ft, &mut fr);
        assert!(events.is_empty());
    }

    // ========================================================================
    // 3b. process_interactions_event — step.start handling
    //     step.start carries initial model_output text and thought signatures.
    // ========================================================================

    #[test]
    fn test_process_step_start_extracts_initial_text() {
        let event = InteractionStreamEvent {
            event_type: "step.start".to_string(),
            index: Some(1),
            delta: None,
            content: None,
            interaction: None,
            step: Some(json!({
                "type": "model_output",
                "content": [{"text": "Once upon", "type": "text"}]
            })),
        };

        let mut full_text = String::new();
        let mut full_reasoning = String::new();
        let events = process_interactions_event(&event, &mut full_text, &mut full_reasoning);

        assert_eq!(full_text, "Once upon");
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], AgentEvent::ResponseChunk(t) if t == "Once upon"));
    }

    #[test]
    fn test_process_step_start_extracts_thought_signature() {
        let event = InteractionStreamEvent {
            event_type: "step.start".to_string(),
            index: Some(0),
            delta: None,
            content: None,
            interaction: None,
            step: Some(json!({
                "type": "thought",
                "signature": "abc123..."
            })),
        };

        let mut full_text = String::new();
        let mut full_reasoning = String::new();
        let events = process_interactions_event(&event, &mut full_text, &mut full_reasoning);

        assert_eq!(events.len(), 1);
        if let AgentEvent::InteractionToolCall { signature, .. } = &events[0] {
            assert_eq!(signature.as_deref(), Some("abc123..."));
        } else {
            panic!("Expected InteractionToolCall with signature");
        }
    }

    #[test]
    fn test_process_step_start_no_payload_produces_nothing() {
        let event = InteractionStreamEvent {
            event_type: "step.start".to_string(),
            index: Some(0),
            delta: None,
            content: None,
            interaction: None,
            step: None,
        };

        let mut ft = String::new();
        let mut fr = String::new();
        let events = process_interactions_event(&event, &mut ft, &mut fr);
        assert!(events.is_empty());
    }

    // ========================================================================
    // 4. Unary Response — Steps JSON Shape Assertions
    //    Validates that the new JSON shapes are structurally correct. These are
    //    shape assertions against serde_json::Value, not typed deserialization.
    // ========================================================================

    #[test]
    fn test_steps_response_shape_basic_text() {
        let body: Value = json!({
            "id": "int_123",
            "steps": [{
                "type": "model_output",
                "status": "done",
                "content": [{ "type": "text", "text": "Why did the chicken cross the road?" }]
            }]
        });
        let steps = body["steps"].as_array().expect("steps should be array");
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0]["type"], "model_output");
        assert_eq!(
            steps[0]["content"][0]["text"],
            "Why did the chicken cross the road?"
        );
    }

    #[test]
    fn test_steps_response_shape_with_thought() {
        let body: Value = json!({
            "id": "int_001",
            "status": "requires_action",
            "steps": [
                {
                    "type": "thought",
                    "summary": [{ "type": "text", "text": "I need to check the weather..." }],
                    "signature": "abc123..."
                },
                { "type": "function_call", "id": "fc_1", "name": "get_weather", "arguments": { "location": "Boston" } }
            ]
        });
        let steps = body["steps"].as_array().unwrap();
        assert_eq!(steps.len(), 2);
        // Thought summary is now an array, not a flat string
        let summary = steps[0]["summary"]
            .as_array()
            .expect("summary should be array");
        assert_eq!(summary[0]["text"], "I need to check the weather...");
        assert_eq!(steps[1]["type"], "function_call");
    }

    #[test]
    fn test_steps_response_shape_function_result_then_output() {
        let body: Value = json!({
            "id": "int_002",
            "steps": [
                { "type": "function_result", "call_id": "fc_1", "name": "get_weather", "result": [{ "type": "text", "text": "52°F" }] },
                { "type": "model_output", "content": [{ "type": "text", "text": "It's 52°F." }] }
            ]
        });
        let steps = body["steps"].as_array().unwrap();
        assert_eq!(steps[0]["type"], "function_result");
        assert_eq!(steps[1]["content"][0]["text"], "It's 52°F.");
    }

    #[test]
    fn test_steps_response_shape_google_search() {
        let body: Value = json!({
            "id": "int_456",
            "steps": [
                { "type": "google_search_call", "id": "gs_1", "arguments": { "queries": ["test"] } },
                { "type": "google_search_result", "call_id": "gs_1", "result": { "search_suggestions": "..." } },
                { "type": "model_output", "content": [{ "type": "text", "text": "Result.", "annotations": [{ "type": "url_citation", "url": "https://example.com" }] }] }
            ]
        });
        let steps = body["steps"].as_array().unwrap();
        assert_eq!(steps.len(), 3);
        assert_eq!(
            steps[2]["content"][0]["annotations"][0]["type"],
            "url_citation"
        );
    }

    // ========================================================================
    // 5. extract_model_text_from_steps — production helper tests
    //    Tests the shared helper used by both background.rs and these tests.
    // ========================================================================

    #[test]
    fn test_extract_model_text_simple() {
        let body = json!({
            "id": "int_1",
            "steps": [{ "type": "model_output", "content": [{ "type": "text", "text": "The answer is 42." }] }]
        });
        assert_eq!(
            extract_model_text_from_steps(&body),
            Some("The answer is 42.".to_string())
        );
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
        assert_eq!(
            extract_model_text_from_steps(&body),
            Some("Final answer.".to_string())
        );
    }

    #[test]
    fn test_extract_model_text_concatenates_multi_part() {
        let body = json!({
            "steps": [{
                "type": "model_output",
                "content": [
                    { "type": "text", "text": "Part one. " },
                    { "type": "text", "text": "Part two." }
                ]
            }]
        });
        assert_eq!(
            extract_model_text_from_steps(&body),
            Some("Part one. Part two.".to_string())
        );
    }

    #[test]
    fn test_extract_model_text_returns_none_for_no_model_output() {
        let body = json!({
            "steps": [{ "type": "function_call", "id": "fc_1", "name": "get_weather", "arguments": {} }]
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
        let body = json!({ "outputs": [{ "type": "text", "text": "legacy" }] });
        assert_eq!(extract_model_text_from_steps(&body), None);
    }

    // ========================================================================
    // 6. Full streaming session simulations
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
            r#"data: {"event_type":"interaction.completed","interaction":{"id":"int_xyz","status":"completed"}}"#,
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
        // step.start delivers "Once upon", step.delta appends " a time..."
        assert_eq!(full_text, "Once upon a time...");
        // signature + reasoning + initial text + delta text = 4 events
        assert_eq!(all_events.len(), 4);
    }

    #[test]
    fn test_full_streaming_session_with_tool_call() {
        let sse_lines = vec![
            r#"data: {"event_type":"interaction.created","interaction":{"id":"int_xyz","status":"in_progress"}}"#,
            r#"data: {"event_type":"step.start","index":0,"step":{"type":"thought"}}"#,
            r#"data: {"event_type":"step.delta","index":0,"delta":{"type":"thought","thought":"I'll call get_weather."}}"#,
            r#"data: {"event_type":"step.stop","index":0,"status":"done"}"#,
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
        assert!(full_text.is_empty());

        // 1 reasoning + 1 tool call = 2 events
        assert_eq!(all_events.len(), 2);
        let tool_event = &all_events[1];
        if let AgentEvent::InteractionToolCall {
            id,
            name,
            arguments,
            ..
        } = tool_event
        {
            assert_eq!(id, "fc_1");
            assert_eq!(name, "get_weather");
            assert_eq!(arguments["location"], "Boston, MA");
        } else {
            panic!("Expected InteractionToolCall event");
        }
    }
}
