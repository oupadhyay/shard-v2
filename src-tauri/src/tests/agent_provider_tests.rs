//! Phase 4 — provider turn handlers + classify_intent + transcript summary.
//!
//! Each turn handler is exercised end-to-end against `wiremock`, asserting on:
//!   * emitted Tauri events (response/reasoning/tool-call/error)
//!   * mutations to the in-memory `history` Vec
//!   * the Ok(true)/Ok(false) "should-continue" return value
//!
//! The classify_intent + summarize_long_transcript tests prove the
//! `crate::endpoints::gemini_classify` override hooks the real call paths.

#![cfg(test)]

use crate::agent::{Agent, ChatMessage, TurnContext};
use crate::tests::agent_helpers::{
    agent_test_lock, captured, gemini_sse, openrouter_sse, or_delta_text, register_listeners,
    TestEnv,
};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

// ============================================================================
// Helpers
// ============================================================================

fn user(s: &str) -> ChatMessage {
    ChatMessage {
        role: "user".into(),
        content: Some(s.into()),
        reasoning: None,
        tool_calls: None,
        tool_call_id: None,
        is_cron: None,
        images: None,
    }
}

fn config_with_gemini() -> crate::config::AppConfig {
    let mut c = crate::config::AppConfig::default();
    c.gemini_api_key = Some("test-gemini".into());
    c.selected_model = Some("gemini-3.1-flash-lite-preview".into());
    c.enable_tools = Some(false); // keep request body lean
    c
}

fn config_with_openrouter() -> crate::config::AppConfig {
    let mut c = crate::config::AppConfig::default();
    c.openrouter_api_key = Some("test-openrouter".into());
    c.selected_model = Some("google/gemma-4-31b-it:free".into());
    c.enable_tools = Some(false);
    c
}

fn config_with_groq() -> crate::config::AppConfig {
    let mut c = crate::config::AppConfig::default();
    c.groq_api_key = Some("test-groq".into());
    c.openrouter_api_key = Some("test-openrouter".into()); // for fallback
    c.selected_model = Some("gpt-oss-120b (Groq)".into());
    c.enable_tools = Some(false);
    c
}

// ============================================================================
// classify_intent (3 branches: YES, no-YES, HTTP failure)
// ============================================================================

mod classify_intent {
    use super::*;

    #[tokio::test]
    async fn yes_response_returns_true() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        let agent = Agent::new(env.handle.clone());

        Mock::given(method("POST"))
            .and(path("/v1beta/models/classifier:generateContent"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "candidates": [{
                    "content": {"parts": [{"text": "YES"}]}
                }]
            })))
            .mount(&env.server)
            .await;

        let r = agent
            .classify_intent("research prompt", "test-key")
            .await
            .unwrap();
        assert!(r);
    }

    #[tokio::test]
    async fn no_response_returns_false() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        let agent = Agent::new(env.handle.clone());

        Mock::given(method("POST"))
            .and(path("/v1beta/models/classifier:generateContent"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "candidates": [{
                    "content": {"parts": [{"text": "NO"}]}
                }]
            })))
            .mount(&env.server)
            .await;

        let r = agent
            .classify_intent("just chat", "test-key")
            .await
            .unwrap();
        assert!(!r);
    }

    #[tokio::test]
    async fn http_failure_returns_err() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        let agent = Agent::new(env.handle.clone());

        Mock::given(method("POST"))
            .and(path("/v1beta/models/classifier:generateContent"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&env.server)
            .await;

        let r = agent.classify_intent("anything", "test-key").await;
        assert!(r.is_err());
        assert!(r.unwrap_err().contains("Intent classification failed"));
    }

    #[tokio::test]
    async fn missing_text_field_returns_false() {
        // The function returns false when the response shape doesn't contain
        // a candidates[0].content.parts[0].text path.
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        let agent = Agent::new(env.handle.clone());

        Mock::given(method("POST"))
            .and(path("/v1beta/models/classifier:generateContent"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "candidates": []
            })))
            .mount(&env.server)
            .await;

        let r = agent
            .classify_intent("anything", "test-key")
            .await
            .unwrap();
        assert!(!r);
    }
}

// ============================================================================
// process_gemini_turn (key branches)
// ============================================================================

mod gemini_turn {
    use super::*;

    /// HTTP non-success → emits agent-error event AND returns Err.
    #[tokio::test]
    async fn http_failure_emits_error_and_returns_err() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        let agent = Agent::new(env.handle.clone());
        let cap = captured();
        register_listeners(&env.handle, cap.clone());

        Mock::given(method("POST"))
            .and(path("/v1beta/interactions"))
            .respond_with(ResponseTemplate::new(503).set_body_string("upstream busy"))
            .mount(&env.server)
            .await;

        let mut history = vec![user("hi")];
        let config = config_with_gemini();
        let r = agent
            .process_gemini_turn(&env.handle, &config, &mut history, 1, &TurnContext::default())
            .await;

        assert!(r.is_err());
        let err_msg = r.unwrap_err();
        assert!(err_msg.contains("503"), "{err_msg}");

        // Allow async listener flush.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let snapshot = cap.lock().unwrap().clone();
        assert!(!snapshot.errors.is_empty(), "agent-error must be emitted");
    }

    /// Successful response with text content emits chunks and pushes an
    /// assistant message; returns Ok(false) (no tool calls means final).
    #[tokio::test]
    async fn text_response_emits_chunks_and_pushes_assistant_message() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        let agent = Agent::new(env.handle.clone());
        let cap = captured();
        register_listeners(&env.handle, cap.clone());

        // Interactions API streams `content.delta` events with a discriminated
        // `delta` union (see InteractionStreamEvent + InteractionDelta).
        let body = gemini_sse(&[
            json!({
                "event_type": "content.delta",
                "delta": {"type": "text", "text": "Hello"}
            }),
            json!({
                "event_type": "content.delta",
                "delta": {"type": "text", "text": " world"}
            }),
        ]);

        Mock::given(method("POST"))
            .and(path("/v1beta/interactions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(body),
            )
            .mount(&env.server)
            .await;

        let mut history = vec![user("hi")];
        let config = config_with_gemini();
        let r = agent
            .process_gemini_turn(&env.handle, &config, &mut history, 1, &TurnContext::default())
            .await;

        // No tool calls → Ok(false), assistant message pushed with concatenated text.
        assert_eq!(r.unwrap(), false);
        assert_eq!(history.len(), 2);
        let assistant = history.last().unwrap();
        assert_eq!(assistant.role, "assistant");
        assert_eq!(assistant.content.as_deref(), Some("Hello world"));

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let snapshot = cap.lock().unwrap().clone();
        assert!(snapshot.responses.contains(&"Hello".to_string()));
        assert!(snapshot.responses.contains(&" world".to_string()));
    }

    /// Missing API key short-circuits without any HTTP call.
    #[tokio::test]
    async fn missing_api_key_returns_err_immediately() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        let agent = Agent::new(env.handle.clone());
        let mut config = crate::config::AppConfig::default();
        config.selected_model = Some("gemini-3.1-flash-lite-preview".into());
        let mut history = vec![user("hi")];
        let r = agent
            .process_gemini_turn(&env.handle, &config, &mut history, 1, &TurnContext::default())
            .await;
        assert!(r.is_err());
        assert!(r.unwrap_err().contains("No Gemini API key"));
        // History untouched.
        assert_eq!(history.len(), 1);
    }
}

// ============================================================================
// process_openrouter_turn (key branches)
// ============================================================================

mod openrouter_turn {
    use super::*;

    #[tokio::test]
    async fn missing_provider_key_propagates_err() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        let agent = Agent::new(env.handle.clone());

        // No openrouter_api_key in config → get_model_provider_config errs.
        let mut config = crate::config::AppConfig::default();
        config.selected_model = Some("google/gemma-4-31b-it:free".into());
        let mut history = vec![user("hi")];
        let r = agent
            .process_openrouter_turn(
                &env.handle,
                &config,
                &mut history,
                1,
                &TurnContext::default(),
            )
            .await;
        assert!(r.is_err());
        let err = r.unwrap_err();
        assert!(err.contains("OpenRouter") || err.contains("API key"), "{err}");
    }

    #[tokio::test]
    async fn text_only_stream_pushes_assistant_message_and_returns_ok_false() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        let agent = Agent::new(env.handle.clone());
        let cap = captured();
        register_listeners(&env.handle, cap.clone());

        let body = openrouter_sse(&[or_delta_text("Hi"), or_delta_text(" there")]);

        Mock::given(method("POST"))
            .and(path("/openrouter/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(body),
            )
            .mount(&env.server)
            .await;

        let mut history = vec![user("hi")];
        let config = config_with_openrouter();
        let r = agent
            .process_openrouter_turn(
                &env.handle,
                &config,
                &mut history,
                1,
                &TurnContext::default(),
            )
            .await
            .unwrap();
        assert!(!r, "no tool calls → continue=false");

        // History grew by exactly one assistant message.
        assert_eq!(history.len(), 2);
        let assistant = history.last().unwrap();
        assert_eq!(assistant.role, "assistant");
        assert_eq!(assistant.content.as_deref(), Some("Hi there"));

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let snapshot = cap.lock().unwrap().clone();
        assert!(snapshot.responses.contains(&"Hi".to_string()));
        assert!(snapshot.responses.contains(&" there".to_string()));
    }

    #[tokio::test]
    async fn empty_stream_returns_ok_false_and_does_not_push() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        let agent = Agent::new(env.handle.clone());

        let body = openrouter_sse(&[]); // just data: [DONE]

        Mock::given(method("POST"))
            .and(path("/openrouter/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(body),
            )
            .mount(&env.server)
            .await;

        let mut history = vec![user("hi")];
        let r = agent
            .process_openrouter_turn(
                &env.handle,
                &config_with_openrouter(),
                &mut history,
                1,
                &TurnContext::default(),
            )
            .await
            .unwrap();
        assert!(!r);
        // No assistant message pushed because content+reasoning+tool_calls are all empty.
        assert_eq!(history.len(), 1);
    }

    #[tokio::test]
    async fn http_failure_non_quota_returns_err_and_emits_error_event() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        let agent = Agent::new(env.handle.clone());
        let cap = captured();
        register_listeners(&env.handle, cap.clone());

        Mock::given(method("POST"))
            .and(path("/openrouter/chat/completions"))
            .respond_with(ResponseTemplate::new(500).set_body_string("internal error"))
            .mount(&env.server)
            .await;

        let mut history = vec![user("hi")];
        let r = agent
            .process_openrouter_turn(
                &env.handle,
                &config_with_openrouter(),
                &mut history,
                1,
                &TurnContext::default(),
            )
            .await;
        assert!(r.is_err());

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(!cap.lock().unwrap().errors.is_empty());
    }

    #[tokio::test]
    async fn groq_quota_error_with_openrouter_key_falls_back() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        let agent = Agent::new(env.handle.clone());
        let cap = captured();
        register_listeners(&env.handle, cap.clone());

        // First request: Groq returns a quota error.
        Mock::given(method("POST"))
            .and(path("/groq/chat/completions"))
            .respond_with(ResponseTemplate::new(429).set_body_string("token_quota_exceeded"))
            .mount(&env.server)
            .await;

        // Fallback request: OpenRouter responds OK with one chunk.
        let body = openrouter_sse(&[or_delta_text("fallback-content")]);
        Mock::given(method("POST"))
            .and(path("/openrouter/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(body),
            )
            .mount(&env.server)
            .await;

        let mut history = vec![user("hi")];
        let r = agent
            .process_openrouter_turn(
                &env.handle,
                &config_with_groq(),
                &mut history,
                1,
                &TurnContext::default(),
            )
            .await
            .unwrap();
        assert!(!r);
        assert_eq!(history.len(), 2);
        assert_eq!(
            history.last().unwrap().content.as_deref(),
            Some("fallback-content")
        );

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let snapshot = cap.lock().unwrap().clone();
        assert!(
            !snapshot.fallbacks.is_empty(),
            "agent-fallback must be emitted on quota → openrouter switch"
        );
    }

    #[tokio::test]
    async fn groq_quota_without_openrouter_key_returns_err() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        let agent = Agent::new(env.handle.clone());

        Mock::given(method("POST"))
            .and(path("/groq/chat/completions"))
            .respond_with(ResponseTemplate::new(429).set_body_string("token_quota_exceeded"))
            .mount(&env.server)
            .await;

        let mut config = crate::config::AppConfig::default();
        config.groq_api_key = Some("test-groq".into());
        // NOTE: no openrouter_api_key.
        config.selected_model = Some("gpt-oss-120b (Groq)".into());

        let mut history = vec![user("hi")];
        let r = agent
            .process_openrouter_turn(
                &env.handle,
                &config,
                &mut history,
                1,
                &TurnContext::default(),
            )
            .await;
        assert!(r.is_err());
        assert!(r.unwrap_err().contains("token_quota_exceeded"));
    }
}
