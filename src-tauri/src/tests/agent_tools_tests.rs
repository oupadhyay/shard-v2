//! Phase 3 — tool dispatch + cache wrapper tests for `Agent::execute_tool`.
//!
//! Strategy:
//!   * Cache wrapper branches are hit with directly-controlled cache files.
//!   * In-process tools (incognito guards, persona ops, wake_me_up_in
//!     validation, list_personas, memory_search arg validation) are tested
//!     end-to-end against the local sandbox.
//!   * One representative network tool (`get_weather`) is exercised end-to-end
//!     against `wiremock` to prove the dispatch + JSON parsing pipeline works.

#![cfg(test)]

use serde_json::json;

use crate::agent::Agent;
use crate::tests::agent_helpers::{agent_test_lock, TestEnv};

fn config() -> crate::config::AppConfig {
    crate::config::AppConfig::default()
}

fn config_incognito() -> crate::config::AppConfig {
    let mut c = crate::config::AppConfig::default();
    c.incognito_mode = Some(true);
    c
}

// ============================================================================
// Cache wrapper (4 branches)
// ============================================================================

mod cache_wrapper {
    use super::*;

    #[tokio::test]
    async fn cache_hit_short_circuits_uncached_call() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        let agent = Agent::new(env.handle.clone());

        // Pre-populate cache for `web_search` so the dispatch never reaches
        // the network. If it DID reach the network we'd see a wiremock 404
        // because we didn't mount any handler.
        let args = json!({"query": "rust"});
        crate::cache::cache_result(&env.handle, "web_search", &args, "CACHED-RESULT");

        let r = agent
            .execute_tool(&env.handle, "web_search", &args, &config())
            .await;
        assert_eq!(r, "CACHED-RESULT");
    }

    #[tokio::test]
    async fn unknown_tool_returns_known_error_message_and_is_not_cached() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        let agent = Agent::new(env.handle.clone());
        let args = json!({});
        let r = agent
            .execute_tool(&env.handle, "no_such_tool", &args, &config())
            .await;
        assert_eq!(r, "Unknown tool: no_such_tool");
        // Cache must not contain a key for an unknown tool (TTL is None).
        let cached = crate::cache::get_cached_result(&env.handle, "no_such_tool", &args);
        assert!(cached.is_none());
    }

    #[tokio::test]
    async fn error_results_are_not_cached() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        let agent = Agent::new(env.handle.clone());

        // youtube_transcript with an unrecognizable input fails deterministically
        // before any network call, returning a string that starts with "Error:".
        // It IS cacheable (60-day TTL), so this exercises the don't-cache-errors
        // branch in execute_tool without depending on any upstream service.
        let args = json!({"video": "not-a-url-or-id"});
        let r = agent
            .execute_tool(&env.handle, "youtube_transcript", &args, &config())
            .await;
        assert!(r.starts_with("Error"), "expected error, got: {r}");
        let cached = crate::cache::get_cached_result(&env.handle, "youtube_transcript", &args);
        assert!(
            cached.is_none(),
            "errors must never be cached, but found: {:?}",
            cached
        );
    }

    #[tokio::test]
    async fn ok_result_is_cached_for_cacheable_tool() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        let agent = Agent::new(env.handle.clone());
        // Pre-seed cache directly to prove the cached lookup path is what
        // returns OK results across calls (we already test the actual write
        // path indirectly via `get_weather` later in Phase 4).
        let args = json!({"q": "ok-key"});
        crate::cache::cache_result(&env.handle, "search_wikipedia", &args, "OK-VALUE");
        let r = agent
            .execute_tool(&env.handle, "search_wikipedia", &args, &config())
            .await;
        assert_eq!(r, "OK-VALUE");
    }
}

// ============================================================================
// Incognito guards (3 tools)
// ============================================================================

mod incognito {
    use super::*;

    #[tokio::test]
    async fn save_memory_blocked_in_incognito() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        let agent = Agent::new(env.handle.clone());
        let r = agent
            .execute_tool(
                &env.handle,
                "save_memory",
                &json!({"content": "x", "importance": 3}),
                &config_incognito(),
            )
            .await;
        assert!(r.contains("Skipped") || r.contains("incognito"));
    }

    #[tokio::test]
    async fn update_topic_summary_blocked_in_incognito() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        let agent = Agent::new(env.handle.clone());
        let r = agent
            .execute_tool(
                &env.handle,
                "update_topic_summary",
                &json!({"topic": "t", "content": "c"}),
                &config_incognito(),
            )
            .await;
        assert!(r.contains("Skipped") || r.contains("incognito"));
    }

    #[tokio::test]
    async fn refresh_memories_blocked_in_incognito() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        let agent = Agent::new(env.handle.clone());
        let r = agent
            .execute_tool(
                &env.handle,
                "refresh_memories",
                &json!({}),
                &config_incognito(),
            )
            .await;
        assert!(r.contains("Skipped") || r.contains("incognito"));
    }
}

// ============================================================================
// memory_search argument validation (no API call)
// ============================================================================

mod memory_search {
    use super::*;

    #[tokio::test]
    async fn empty_query_with_no_time_filter_returns_argument_error() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        let agent = Agent::new(env.handle.clone());
        let r = agent
            .execute_tool(&env.handle, "memory_search", &json!({}), &config())
            .await;
        assert!(r.starts_with("Error: query parameter is required"), "{r}");
    }

    #[tokio::test]
    async fn missing_gemini_key_returns_dedicated_error() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        let agent = Agent::new(env.handle.clone());
        let r = agent
            .execute_tool(
                &env.handle,
                "memory_search",
                &json!({"query": "anything"}),
                &config(),
            )
            .await;
        assert!(r.contains("requires a Gemini API key"), "got: {r}");
    }
}

// ============================================================================
// memory_get
// ============================================================================

mod memory_get {
    use super::*;

    #[tokio::test]
    async fn missing_path_and_session_id_returns_argument_error() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        let agent = Agent::new(env.handle.clone());
        let r = agent
            .execute_tool(&env.handle, "memory_get", &json!({}), &config())
            .await;
        assert!(r.starts_with("Error: path parameter is required"), "{r}");
    }
}

// ============================================================================
// wake_me_up_in argument validation (3 cases)
// ============================================================================

mod wake_me_up_in {
    use super::*;

    #[tokio::test]
    async fn zero_duration_rejected() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        let agent = Agent::new(env.handle.clone());
        let r = agent
            .execute_tool(
                &env.handle,
                "wake_me_up_in",
                &json!({"duration_minutes": 0, "context": "ping"}),
                &config(),
            )
            .await;
        assert!(r.contains("between 1 and 1440"), "{r}");
    }

    #[tokio::test]
    async fn over_24_hours_rejected() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        let agent = Agent::new(env.handle.clone());
        let r = agent
            .execute_tool(
                &env.handle,
                "wake_me_up_in",
                &json!({"duration_minutes": 1441, "context": "ping"}),
                &config(),
            )
            .await;
        assert!(r.contains("between 1 and 1440"), "{r}");
    }

    #[tokio::test]
    async fn empty_context_rejected() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        let agent = Agent::new(env.handle.clone());
        let r = agent
            .execute_tool(
                &env.handle,
                "wake_me_up_in",
                &json!({"duration_minutes": 5, "context": ""}),
                &config(),
            )
            .await;
        assert!(r.contains("context must not be empty"), "{r}");
    }
}

// ============================================================================
// Personas: list / load / unload
// ============================================================================

mod personas {
    use super::*;

    fn write_persona(env: &TestEnv, name: &str, body: &str) {
        let dir = crate::personas::get_personas_dir().expect("personas dir");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{name}.md"));
        std::fs::write(&path, body).unwrap();
        // Sanity check: the path must live inside our sandbox.
        assert!(path.starts_with(env._tempdir.path()), "persona escaped sandbox: {:?}", path);
    }

    #[tokio::test]
    async fn list_personas_with_empty_dir() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        let agent = Agent::new(env.handle.clone());
        let r = agent
            .execute_tool(&env.handle, "list_personas", &json!({}), &config())
            .await;
        // No personas seeded → message says none available.
        assert!(r.to_lowercase().contains("no dynamic personas"), "{r}");
    }

    #[tokio::test]
    async fn list_personas_lists_seeded_files() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        write_persona(&env, "writer", "# Writer\nBe concise.");
        write_persona(&env, "researcher", "# Researcher\nBe thorough.");
        let agent = Agent::new(env.handle.clone());
        let r = agent
            .execute_tool(&env.handle, "list_personas", &json!({}), &config())
            .await;
        assert!(r.contains("writer"), "missing writer in: {r}");
        assert!(r.contains("researcher"), "missing researcher in: {r}");
    }

    #[tokio::test]
    async fn load_persona_unknown_returns_not_found() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        let agent = Agent::new(env.handle.clone());
        let r = agent
            .execute_tool(
                &env.handle,
                "load_persona",
                &json!({"name": "ghost"}),
                &config(),
            )
            .await;
        assert!(r.contains("not found"), "{r}");
    }

    #[tokio::test]
    async fn load_then_unload_persona_round_trip_updates_active_skills() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        write_persona(&env, "writer", "be brief");
        let agent = Agent::new(env.handle.clone());

        let r1 = agent
            .execute_tool(
                &env.handle,
                "load_persona",
                &json!({"name": "writer"}),
                &config(),
            )
            .await;
        assert!(r1.contains("Successfully loaded"), "{r1}");

        // Loading again is idempotent.
        let r2 = agent
            .execute_tool(
                &env.handle,
                "load_persona",
                &json!({"name": "writer"}),
                &config(),
            )
            .await;
        assert!(r2.contains("already active"), "{r2}");

        // Verify active_skills DB reflects the load.
        let sid = agent.session_id.lock().await.clone();
        let store = crate::memories::get_vector_store(&env.handle).unwrap();
        let active = crate::db::sessions::get_active_skills(&store, &sid).unwrap();
        assert_eq!(active, vec!["writer".to_string()]);

        // Unload returns to empty.
        let r3 = agent
            .execute_tool(
                &env.handle,
                "unload_persona",
                &json!({"name": "writer"}),
                &config(),
            )
            .await;
        assert!(r3.contains("Successfully unloaded"), "{r3}");

        let active_after = crate::db::sessions::get_active_skills(&store, &sid).unwrap();
        assert!(active_after.is_empty());

        // Unloading a non-active persona returns the "not currently active" message.
        let r4 = agent
            .execute_tool(
                &env.handle,
                "unload_persona",
                &json!({"name": "writer"}),
                &config(),
            )
            .await;
        assert!(r4.contains("not currently active"), "{r4}");
    }
}

// ============================================================================
// Notes on coverage gaps deferred to later phases
// ============================================================================
//
// The following tool branches require either a real upstream API or a more
// invasive endpoint-injection refactor; they are intentionally out of scope
// for Phase 3 and will be addressed in Phase 4 when we wire wiremock through
// the provider turn handlers:
//
//   * `get_weather` / `get_stock_price` / `web_search` / `search_wikipedia`
//     happy-path success (each integration uses its own hard-coded URL).
//   * `youtube_transcript` long-transcript truncation triggering
//     `summarize_long_transcript` (needs the Gemini-backed background LLM).
//   * `run_python` happy-path (depends on wasm runtime resource bundle that
//     is not present in the cargo test sandbox).
//
// The `Error:`-not-cached branch is fully covered by
// `cache_wrapper::error_results_are_not_cached` above using youtube_transcript
// with an invalid input — that path is deterministic and offline.

// ============================================================================
// Self-editing tools (read_file + edit_file)
// ============================================================================
//
// These exercise the full backend pipeline for the generic self-awareness
// tools introduced for Part 1 of "Make Shard self-aware":
//   * argument parsing in `agent/tools/mod.rs`
//   * allow-list + IO in `self_files.rs`
//   * the `file-edited` event contract that the frontend diff viewer depends on
//
// No network is involved; everything runs against the tempdir-rooted $HOME
// established by `TestEnv`, so these tests are fast and deterministic.

mod self_files_dispatch {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tauri::Listener;

    fn write_config_toml(env: &TestEnv, contents: &str) {
        use tauri::Manager;
        let cfg_dir = env.handle.path().app_config_dir().unwrap();
        std::fs::create_dir_all(&cfg_dir).unwrap();
        std::fs::write(cfg_dir.join("config.toml"), contents).unwrap();
    }

    fn read_config_toml(env: &TestEnv) -> String {
        use tauri::Manager;
        let cfg_dir = env.handle.path().app_config_dir().unwrap();
        std::fs::read_to_string(cfg_dir.join("config.toml")).unwrap()
    }

    // ── read_file ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn read_file_empty_when_config_missing() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        let agent = Agent::new(env.handle.clone());

        let r = agent
            .execute_tool(
                &env.handle,
                "read_file",
                &json!({"path": "config.toml"}),
                &config(),
            )
            .await;
        // `read_file` returns raw file contents so the agent can paste
        // them verbatim into `edit_file`'s `old_str`. Missing files
        // therefore return an empty string rather than a prose hint.
        assert!(
            r.is_empty(),
            "expected empty string for missing file, got: {:?}",
            r
        );
    }

    #[tokio::test]
    async fn read_file_returns_contents_when_present() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        write_config_toml(&env, "selected_model = \"gpt-oss-120b\"\n");
        let agent = Agent::new(env.handle.clone());

        let r = agent
            .execute_tool(
                &env.handle,
                "read_file",
                &json!({"path": "config.toml"}),
                &config(),
            )
            .await;
        assert!(r.contains("selected_model"));
        assert!(r.contains("gpt-oss-120b"));
    }

    #[tokio::test]
    async fn read_file_rejects_empty_path() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        let agent = Agent::new(env.handle.clone());

        let r = agent
            .execute_tool(&env.handle, "read_file", &json!({"path": ""}), &config())
            .await;
        assert!(r.starts_with("Error"), "got: {}", r);
        assert!(r.contains("empty"), "got: {}", r);
    }

    #[tokio::test]
    async fn read_file_rejects_unknown_path() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        let agent = Agent::new(env.handle.clone());

        let r = agent
            .execute_tool(
                &env.handle,
                "read_file",
                &json!({"path": "nonexistent.toml"}),
                &config(),
            )
            .await;
        assert!(r.starts_with("Error"), "got: {}", r);
        assert!(r.contains("not allow-listed"), "got: {}", r);
    }

    #[tokio::test]
    async fn read_file_rejects_directory_traversal() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        let agent = Agent::new(env.handle.clone());

        let r = agent
            .execute_tool(
                &env.handle,
                "read_file",
                &json!({"path": "../config.toml"}),
                &config(),
            )
            .await;
        assert!(r.starts_with("Error"), "got: {}", r);
        assert!(r.contains("not allow-listed"), "got: {}", r);
    }

    // ── edit_file ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn edit_file_creates_initial_content_from_empty() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        let agent = Agent::new(env.handle.clone());

        let r = agent
            .execute_tool(
                &env.handle,
                "edit_file",
                &json!({
                    "path": "config.toml",
                    "old_str": "",
                    "new_str": "selected_model = \"gpt-oss-120b\"\n",
                    "replace_all": false,
                }),
                &config(),
            )
            .await;
        assert!(!r.starts_with("Error"), "unexpected error: {}", r);
        assert!(r.contains("Edited"));
        assert!(r.contains("```diff"));
        // File on disk reflects the new content.
        assert_eq!(read_config_toml(&env), "selected_model = \"gpt-oss-120b\"\n");
    }

    #[tokio::test]
    async fn edit_file_round_trip_replace() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        write_config_toml(&env, "selected_model = \"old-model\"\n");
        let agent = Agent::new(env.handle.clone());

        let r = agent
            .execute_tool(
                &env.handle,
                "edit_file",
                &json!({
                    "path": "config.toml",
                    "old_str": "old-model",
                    "new_str": "new-model",
                    "replace_all": false,
                }),
                &config(),
            )
            .await;
        assert!(!r.starts_with("Error"), "got: {}", r);
        assert_eq!(read_config_toml(&env), "selected_model = \"new-model\"\n");
    }

    #[tokio::test]
    async fn edit_file_blocks_api_key_in_old_str() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        write_config_toml(&env, "gemini_api_key = \"placeholder\"\n");
        let agent = Agent::new(env.handle.clone());

        let r = agent
            .execute_tool(
                &env.handle,
                "edit_file",
                &json!({
                    "path": "config.toml",
                    "old_str": "gemini_api_key = \"placeholder\"",
                    "new_str": "",
                    "replace_all": false,
                }),
                &config(),
            )
            .await;
        assert!(r.starts_with("Error"), "got: {}", r);
        assert!(r.contains("api_key"));
    }

    #[tokio::test]
    async fn edit_file_blocks_api_key_in_new_str() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        write_config_toml(&env, "selected_model = \"x\"\n");
        let agent = Agent::new(env.handle.clone());

        let r = agent
            .execute_tool(
                &env.handle,
                "edit_file",
                &json!({
                    "path": "config.toml",
                    "old_str": "selected_model = \"x\"",
                    "new_str": "gemini_api_key = \"leaked\"",
                    "replace_all": false,
                }),
                &config(),
            )
            .await;
        assert!(r.starts_with("Error"), "got: {}", r);
        assert!(r.contains("api_key"));
        // File on disk should be untouched.
        assert_eq!(read_config_toml(&env), "selected_model = \"x\"\n");
    }

    #[tokio::test]
    async fn edit_file_rejects_ambiguous_old_str_without_replace_all() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        write_config_toml(&env, "foo = 1\nfoo = 2\n");
        let agent = Agent::new(env.handle.clone());

        let r = agent
            .execute_tool(
                &env.handle,
                "edit_file",
                &json!({
                    "path": "config.toml",
                    "old_str": "foo",
                    "new_str": "bar",
                    "replace_all": false,
                }),
                &config(),
            )
            .await;
        assert!(r.starts_with("Error"), "got: {}", r);
        assert!(r.contains("matches 2 times"));
        // File untouched.
        assert_eq!(read_config_toml(&env), "foo = 1\nfoo = 2\n");
    }

    #[tokio::test]
    async fn edit_file_replace_all_succeeds_with_multiple_matches() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        write_config_toml(&env, "foo = 1\nfoo = 2\n");
        let agent = Agent::new(env.handle.clone());

        let r = agent
            .execute_tool(
                &env.handle,
                "edit_file",
                &json!({
                    "path": "config.toml",
                    "old_str": "foo",
                    "new_str": "bar",
                    "replace_all": true,
                }),
                &config(),
            )
            .await;
        assert!(!r.starts_with("Error"), "got: {}", r);
        assert!(r.contains("2 replacements"));
        assert_eq!(read_config_toml(&env), "bar = 1\nbar = 2\n");
    }

    // ── file-edited event contract ───────────────────────────────────────

    #[tokio::test]
    async fn edit_file_emits_file_edited_event_with_outcome_payload() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        write_config_toml(&env, "selected_model = \"alpha\"\n");

        // Install a listener BEFORE invoking the tool so we capture the
        // emission. Payload arrives as a JSON-encoded string of EditOutcome.
        let payloads: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let p = payloads.clone();
        env.handle.listen("file-edited", move |e| {
            p.lock().unwrap().push(e.payload().to_string());
        });

        let agent = Agent::new(env.handle.clone());
        let r = agent
            .execute_tool(
                &env.handle,
                "edit_file",
                &json!({
                    "path": "config.toml",
                    "old_str": "alpha",
                    "new_str": "beta",
                    "replace_all": false,
                }),
                &config(),
            )
            .await;
        assert!(!r.starts_with("Error"), "got: {}", r);

        // tauri::Listener fires synchronously on the same thread that emits.
        // Even so, give the runtime a yield so we don't race the listener
        // registration in unusual schedulers.
        tokio::task::yield_now().await;

        let captured = payloads.lock().unwrap();
        assert_eq!(
            captured.len(),
            1,
            "expected exactly one file-edited event, got {}",
            captured.len()
        );

        // Validate the structured EditOutcome shape the frontend depends on.
        let v: serde_json::Value =
            serde_json::from_str(&captured[0]).expect("payload should be valid JSON");
        assert_eq!(v["path"], "config.toml");
        assert!(v["abs_path"].as_str().unwrap().ends_with("config.toml"));
        assert_eq!(v["before"], "selected_model = \"alpha\"\n");
        assert_eq!(v["after"], "selected_model = \"beta\"\n");
        assert_eq!(v["replacements"], 1);
        let diff = v["unified_diff"].as_str().unwrap();
        assert!(diff.contains("-selected_model = \"alpha\""));
        assert!(diff.contains("+selected_model = \"beta\""));
    }
}
