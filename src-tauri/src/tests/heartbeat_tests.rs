/**
 * Heartbeat engine tests
 *
 * Tests for heartbeat spec parsing, rate limiting, proactive queue operations,
 * and cron-to-heartbeat migration. All tests are self-contained and use
 * in-memory/tempdir fixtures.
 */
use crate::heartbeat::*;
use crate::tests::agent_helpers::{home_lock_async, HomeJail};
use tauri::Manager;

// ============================================================================
// Spec Parsing Tests
// ============================================================================

#[test]
fn test_parse_heartbeat_spec_valid() {
    let content = r#"
schedule = "0 */2 * * *"
session = "agent:news"
persona = "news-analyst"
max_tool_calls = 3
max_runs_per_day = 6
prompt = "Check the latest headlines for topics I've expressed interest in.\nIf anything is significant, draft a notification."
"#;

    let spec = parse_heartbeat_spec(content, "news-monitor").unwrap();

    assert_eq!(spec.schedule, "0 */2 * * *");
    assert_eq!(spec.session, "agent:news");
    assert_eq!(spec.persona.as_deref(), Some("news-analyst"));
    assert_eq!(spec.max_tool_calls, 3);
    assert_eq!(spec.max_runs_per_day, Some(6));
    assert!(spec.prompt.contains("headlines"));
    assert_eq!(spec.filename, "news-monitor");
}

#[test]
fn test_parse_heartbeat_spec_minimal() {
    let content = r#"
schedule = "0 8 * * *"
session = "agent:morning"
prompt = "Good morning check."
"#;

    let spec = parse_heartbeat_spec(content, "minimal").unwrap();

    assert_eq!(spec.schedule, "0 8 * * *");
    assert_eq!(spec.session, "agent:morning");
    assert!(spec.persona.is_none());
    assert_eq!(spec.max_tool_calls, 5); // default
    assert_eq!(spec.max_runs_per_day, Some(10)); // default
    assert_eq!(spec.prompt, "Good morning check.");
}

#[test]
fn test_parse_heartbeat_spec_missing_schedule() {
    let content = r#"
session = "agent:news"
prompt = "Some prompt."
"#;

    let result = parse_heartbeat_spec(content, "no-schedule");
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("schedule"));
}

#[test]
fn test_parse_heartbeat_spec_missing_session() {
    let content = r#"
schedule = "0 * * * *"
prompt = "Some prompt."
"#;

    let result = parse_heartbeat_spec(content, "no-session");
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("session"));
}

#[test]
fn test_parse_heartbeat_spec_invalid_toml() {
    let content = r#"
schedule: "0 * * * *"
session = "agent:test
"#;

    let result = parse_heartbeat_spec(content, "invalid-toml");
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("parsing TOML"));
}

#[test]
fn test_parse_heartbeat_spec_empty_prompt() {
    let content = r#"
schedule = "0 * * * *"
session = "agent:test"
prompt = "   "
"#;

    let result = parse_heartbeat_spec(content, "empty-prompt");
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("empty prompt"));
}

#[test]
fn test_parse_heartbeat_spec_missing_prompt() {
    let content = r#"
schedule = "0 * * * *"
session = "agent:test"
"#;

    let result = parse_heartbeat_spec(content, "missing-prompt");
    assert!(result.is_err());
    // Error comes from TOML parser missing the 'prompt' field
    assert!(result.unwrap_err().contains("prompt"));
}

// ============================================================================
// Rate Limiter Tests
// ============================================================================

#[test]
fn test_rate_limiter_cooldown() {
    let limiter = HeartbeatRateLimiter::new(60); // 60s cooldown

    let spec = HeartbeatSpec {
        schedule: "0 * * * *".to_string(),
        session: "test:limiter".to_string(),
        persona: None,
        max_tool_calls: 5,
        max_runs_per_day: None,
        prompt: "test".to_string(),
        filename: "test".to_string(),
    };

    // First check: not rate limited
    assert!(!limiter.should_skip(&spec));

    // Record a run
    limiter.record_run(&spec.session);

    // Immediately after: should be rate limited (within 60s cooldown)
    assert!(limiter.should_skip(&spec));
}

#[test]
fn test_rate_limiter_daily_cap() {
    let limiter = HeartbeatRateLimiter::new(0); // No cooldown for this test

    let spec = HeartbeatSpec {
        schedule: "0 * * * *".to_string(),
        session: "test:daily-cap".to_string(),
        persona: None,
        max_tool_calls: 5,
        max_runs_per_day: Some(2), // Cap at 2 per day
        prompt: "test".to_string(),
        filename: "test".to_string(),
    };

    // First run
    assert!(!limiter.should_skip(&spec));
    limiter.record_run(&spec.session);

    // Second run
    assert!(!limiter.should_skip(&spec));
    limiter.record_run(&spec.session);

    // Third run: cap reached
    assert!(limiter.should_skip(&spec));
}

#[test]
fn test_rate_limiter_backoff() {
    let limiter = HeartbeatRateLimiter::new(0); // No cooldown

    let spec = HeartbeatSpec {
        schedule: "0 * * * *".to_string(),
        session: "test:backoff".to_string(),
        persona: None,
        max_tool_calls: 5,
        max_runs_per_day: None,
        prompt: "test".to_string(),
        filename: "test".to_string(),
    };

    // Record a quota error
    limiter.record_quota_error(&spec.session);

    // Should be in backoff
    assert!(limiter.should_skip(&spec));
}

#[test]
fn test_rate_limiter_backoff_clears_on_success() {
    let limiter = HeartbeatRateLimiter::new(0);

    let spec = HeartbeatSpec {
        schedule: "0 * * * *".to_string(),
        session: "test:backoff-clear".to_string(),
        persona: None,
        max_tool_calls: 5,
        max_runs_per_day: None,
        prompt: "test".to_string(),
        filename: "test".to_string(),
    };

    limiter.record_quota_error(&spec.session);
    assert!(limiter.should_skip(&spec));

    // Successful run should clear backoff
    limiter.record_run(&spec.session);
    assert!(!limiter.should_skip(&spec));
}

#[test]
fn test_rate_limiter_no_cap_unlimited() {
    let limiter = HeartbeatRateLimiter::new(0);

    let spec = HeartbeatSpec {
        schedule: "0 * * * *".to_string(),
        session: "test:unlimited".to_string(),
        persona: None,
        max_tool_calls: 5,
        max_runs_per_day: None, // Explicitly no cap
        prompt: "test".to_string(),
        filename: "test".to_string(),
    };

    // Run many times - should never be capped (no daily limit)
    for _ in 0..20 {
        limiter.record_run(&spec.session);
    }
    assert!(!limiter.should_skip(&spec));
}

#[test]
fn test_rate_limiter_default_cap() {
    let limiter = HeartbeatRateLimiter::new(0);

    let spec = HeartbeatSpec {
        schedule: "0 * * * *".to_string(),
        session: "test:default-cap".to_string(),
        persona: None,
        max_tool_calls: 5,
        max_runs_per_day: Some(10), // The new default
        prompt: "test".to_string(),
        filename: "test".to_string(),
    };

    for _ in 0..10 {
        limiter.record_run(&spec.session);
    }

    // 11th run should fail
    assert!(limiter.should_skip(&spec));
}

// ============================================================================
// Cron Migration Tests
// ============================================================================

#[test]
fn test_cron_migration_format() {
    // Test that the migration produces valid TOML heartbeat spec content
    let schedule = "0 */6 * * *";
    let prompt = "Check for system updates and report.";
    let session_idx = 1;

    let content = format!(
        "schedule = \"{}\"\nsession = \"agent:cron-migrated-{}\"\nprompt = \"\"\"{}\"\"\"\n",
        schedule, session_idx, prompt
    );

    // Verify the generated content is a valid heartbeat spec
    let spec = parse_heartbeat_spec(&content, "migrated-cron-1").unwrap();
    assert_eq!(spec.schedule, schedule);
    assert_eq!(spec.session, "agent:cron-migrated-1");
    assert_eq!(spec.prompt, prompt);
}

// ============================================================================
// Spec Discovery Tests
// ============================================================================

#[test]
fn test_load_heartbeat_specs_from_directory() {
    use std::fs;
    let temp_dir = tempfile::TempDir::new().expect("Failed to create temp dir");
    let heartbeats_dir = temp_dir.path().join("heartbeats");
    fs::create_dir_all(&heartbeats_dir).expect("Failed to create heartbeats dir");

    // Write valid spec
    fs::write(
        heartbeats_dir.join("news.toml"),
        "schedule = \"0 */2 * * *\"\nsession = \"agent:news\"\nprompt = \"Check news.\"\n",
    )
    .unwrap();

    // Write another valid spec
    fs::write(
        heartbeats_dir.join("weather.toml"),
        "schedule = \"0 8 * * *\"\nsession = \"agent:weather\"\npersona = \"meteorologist\"\nprompt = \"Get weather forecast.\"\n",
    )
    .unwrap();

    // Write invalid file (bad TOML)
    fs::write(heartbeats_dir.join("invalid.toml"), "No valid toml here.").unwrap();

    // Write non-toml file (should be ignored)
    fs::write(heartbeats_dir.join("notes.txt"), "Some notes").unwrap();

    // Read them back with our parser directly
    let mut specs = Vec::new();
    for entry in fs::read_dir(&heartbeats_dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("toml") {
            let filename = path.file_stem().unwrap().to_str().unwrap().to_string();
            if let Ok(content) = fs::read_to_string(&path) {
                if let Ok(spec) = parse_heartbeat_spec(&content, &filename) {
                    specs.push(spec);
                }
            }
        }
    }

    assert_eq!(specs.len(), 2);
    let sessions: Vec<&str> = specs.iter().map(|s| s.session.as_str()).collect();
    assert!(sessions.contains(&"agent:news"));
    assert!(sessions.contains(&"agent:weather"));
}

// ============================================================================
// Draft-Before-Act Tests
// ============================================================================

#[test]
fn test_is_draft_gated() {
    let reg = crate::tool_registry::global();

    // High-risk heartbeat-mutation tools should be gated
    assert!(reg.is_draft_gated("create_heartbeat"));
    assert!(reg.is_draft_gated("delete_heartbeat"));
    assert!(reg.is_draft_gated("edit_heartbeat"));

    // Valid syntax is not user authorization.
    assert!(reg.is_draft_gated("edit_file"));
    assert!(!reg.is_draft_gated("read_file"));

    // Safe tools should NOT be gated
    assert!(!reg.is_draft_gated("web_search"));
    assert!(!reg.is_draft_gated("save_memory"));
    assert!(!reg.is_draft_gated("run_python"));
    assert!(!reg.is_draft_gated("wake_me_up_in"));
    assert!(!reg.is_draft_gated("load_persona"));
}

#[test]
fn test_draft_payload_serialization() {
    let draft = DraftPayload {
        name: "create_heartbeat".to_string(),
        arguments: serde_json::json!({
            "name": "test-hb",
            "schedule": "0 */4 * * *",
            "session": "agent:test-hb",
            "prompt": "Do something"
        }),
        justification: "User asked me to create a recurring task".to_string(),
        heartbeat_session: "agent:news".to_string(),
    };

    let json = serde_json::to_string(&draft).unwrap();
    let parsed: DraftPayload = serde_json::from_str(&json).unwrap();

    assert_eq!(parsed.name, "create_heartbeat");
    assert_eq!(parsed.arguments["name"], "test-hb");
    assert_eq!(parsed.arguments["schedule"], "0 */4 * * *");
    assert_eq!(parsed.heartbeat_session, "agent:news");
}

#[test]
fn test_heartbeat_tools_include_draft_gated() {
    let tools = crate::tool_registry::global().get_heartbeat_definitions(&[]);
    let tool_names: Vec<&str> = tools.iter().map(|t| t.function.name.as_str()).collect();

    // Should include global tools
    assert!(tool_names.contains(&"web_search"));
    assert!(tool_names.contains(&"save_memory"));
    assert!(tool_names.contains(&"wake_me_up_in"));

    // Should also include draft-gated heartbeat-mutation tools
    assert!(tool_names.contains(&"create_heartbeat"));
    assert!(tool_names.contains(&"delete_heartbeat"));
    assert!(tool_names.contains(&"edit_heartbeat"));

    // Self-awareness file tools are global, so also present here
    assert!(tool_names.contains(&"read_file"));
    assert!(tool_names.contains(&"edit_file"));
}

#[test]
fn test_create_heartbeat_via_draft_tool() {
    // Test the file creation logic from execute_draft_gated_tool
    use std::fs;
    let temp_dir = tempfile::TempDir::new().expect("Failed to create temp dir");
    let heartbeats_dir = temp_dir.path().join("heartbeats");
    fs::create_dir_all(&heartbeats_dir).expect("Failed to create heartbeats dir");

    // Simulate what create_heartbeat does
    let name = "my-test-hb";
    let schedule = "0 */3 * * *";
    let session = "agent:my-test";
    let prompt = "Test heartbeat prompt.";

    let safe_name: String = name
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
        .take(64)
        .collect();

    let filepath = heartbeats_dir.join(format!("{}.toml", safe_name));

    let content = format!(
        "schedule = \"{}\"\nsession = \"{}\"\nprompt = \"\"\"{}\"\"\"\n",
        schedule, session, prompt
    );
    fs::write(&filepath, &content).unwrap();

    // Read it back and parse
    let read = fs::read_to_string(&filepath).unwrap();
    let spec = parse_heartbeat_spec(&read, &safe_name).unwrap();
    assert_eq!(spec.schedule, schedule);
    assert_eq!(spec.session, session);
    assert_eq!(spec.prompt, prompt);
}

#[test]
fn test_filename_sanitization() {
    // Ensure path traversal attacks are blocked
    let malicious = "../../../etc/passwd";
    let safe: String = malicious
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
        .take(64)
        .collect();
    assert_eq!(safe, "etcpasswd");
    assert!(!safe.contains('/'));
    assert!(!safe.contains(".."));
}

#[test]
fn test_normalize_heartbeat_slug() {
    assert_eq!(normalize_heartbeat_slug("Daily_News"), "daily-news");
    assert_eq!(normalize_heartbeat_slug("1review"), "hb-1review");
    assert_eq!(normalize_heartbeat_slug("-foo-bar-"), "foo-bar");
    assert_eq!(normalize_heartbeat_slug("A"), "a-hb");
    let very_long = format!("a{}", "1".repeat(50));
    let normalized = normalize_heartbeat_slug(&very_long);
    assert_eq!(normalized.len(), 41);
}

#[test]
fn test_migrate_legacy_heartbeat_files() {
    use std::fs;
    let temp_dir = tempfile::TempDir::new().expect("Failed to create temp dir");
    let heartbeats_dir = temp_dir.path().join("heartbeats");
    fs::create_dir_all(&heartbeats_dir).expect("Failed to create heartbeats dir");

    // Write a non-conforming spec file
    let legacy_path = heartbeats_dir.join("Daily_News.toml");
    fs::write(
        &legacy_path,
        "schedule = \"0 */2 * * *\"\nsession = \"agent:news\"\nprompt = \"Check news.\"\n",
    )
    .unwrap();

    // Emulate migrate function by scanning the temp dir directly
    let entries = fs::read_dir(&heartbeats_dir).unwrap();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("toml") {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                let normalized = normalize_heartbeat_slug(stem);
                if normalized != stem {
                    let new_path = path.with_file_name(format!("{}.toml", normalized));
                    fs::rename(&path, &new_path).unwrap();
                }
            }
        }
    }

    assert!(!legacy_path.exists());
    assert!(heartbeats_dir.join("daily-news.toml").exists());
}

#[test]
fn test_normalize_heartbeat_slug_edge_cases() {
    let edge_1 = "a---------------------------------------------bc";
    assert_eq!(normalize_heartbeat_slug(edge_1), "a-hb");

    let edge_2 = "-----------------------------------------------";
    assert_eq!(normalize_heartbeat_slug(edge_2), "hb");
}

#[tokio::test]
async fn test_execute_approved_draft_reviewed_check() {
    let _home_lock = home_lock_async().await;
    let _home_jail = HomeJail::new();
    let app = tauri::test::mock_app();
    let handle = app.handle();

    // Set up app config dir so get_heartbeats_dir doesn't fail
    if let Ok(cfg_dir) = handle.path().app_config_dir() {
        let _ = std::fs::create_dir_all(&cfg_dir);
    }

    ensure_proactive_queue_table(handle).unwrap();
    let message_id_str = uuid::Uuid::new_v4().to_string();
    let message_id = &message_id_str;
    let payload = serde_json::json!({
        "name": "create_heartbeat",
        "arguments": {
            "name": "temp-test-hb",
            "schedule": "0 */4 * * *",
            "session": "agent:test-hb",
            "prompt": "Do something test"
        },
        "justification": "Test justification",
        "heartbeat_session": "agent:test",
    });

    let msg = ProactiveMessage {
        id: message_id.to_string(),
        heartbeat_session: "agent:test".to_string(),
        content: "Propose heartbeat".to_string(),
        draft_payload: Some(payload.to_string()),
        needs_approval: true,
        reviewed_at: None,
        approved: None,
        execution_status: None,
        execution_result: None,
        created_at: chrono::Utc::now().to_rfc3339(),
    };

    insert_proactive_message(handle, &msg).unwrap();
    assert!(review_proactive_message(handle, message_id, Some(true)).is_err());
    assert!(review_proactive_message(handle, message_id, None).is_err());
    review_proactive_message(handle, message_id, Some(false)).unwrap();

    // Calling execute_approved_draft on an already reviewed message should return an error!
    let res = execute_approved_draft(handle, message_id).await;
    assert!(res.is_err());
    assert_eq!(
        res.unwrap_err(),
        "Draft has already been reviewed or does not exist"
    );
}

#[tokio::test]
async fn test_execute_draft_gated_tool_validation() {
    let _home_lock = home_lock_async().await;
    let _home_jail = HomeJail::new();
    let app = tauri::test::mock_app();
    let handle = app.handle();

    // Set up app config dir so get_heartbeats_dir doesn't fail
    if let Ok(cfg_dir) = handle.path().app_config_dir() {
        let _ = std::fs::create_dir_all(&cfg_dir);
    }

    // Call with invalid schedule (TOML structure is valid but schedule parsing fails)
    let args = serde_json::json!({
        "name": "valid-name",
        "schedule": "invalid cron schedule",
        "session": "agent:test",
        "prompt": "prompt text"
    });

    let res = execute_draft_gated_tool(handle, "create_heartbeat", &args).await;
    assert!(res.is_err());
    let err = res.unwrap_err();
    assert!(err.contains("schedule") || err.contains("cron") || err.contains("expression"));
}

#[tokio::test]
async fn draft_decisions_survive_failure_and_uncertain_persistence() {
    let _lock = home_lock_async().await;
    let _jail = HomeJail::new();
    let app = tauri::test::mock_app();
    let handle = app.handle();
    ensure_proactive_queue_table(handle).unwrap();
    let store = crate::memories::get_vector_store(handle).unwrap();

    // Malformed payload: consent survives, validation failure is durable.
    let id = queue_tool_draft(handle, "test", "edit_file", &serde_json::json!({}), "test").unwrap();
    store
        .conn
        .execute(
            "UPDATE proactive_queue SET draft_payload = '{' WHERE id = ?1",
            [&id],
        )
        .unwrap();
    assert!(execute_approved_draft(handle, &id)
        .await
        .unwrap_err()
        .contains("Invalid draft"));
    let state = draft_status(handle, &id).unwrap();
    assert_eq!(state["approved"], true);
    assert_eq!(state["execution_status"], "failed");
    assert!(execute_approved_draft(handle, &id).await.is_err());
    assert!(review_proactive_message(handle, &id, Some(false)).is_err());

    // Tool failure is not mistaken for rejection or left only in chat history.
    let id = queue_tool_draft(handle, "test", "edit_file", &serde_json::json!({}), "test").unwrap();
    assert!(execute_approved_draft(handle, &id).await.is_err());
    assert_eq!(
        draft_status(handle, &id).unwrap()["execution_status"],
        "failed"
    );

    // Duplicate clicks share a single durable claim and execute only once.
    let success_id = queue_tool_draft(
        handle,
        "test",
        "edit_file",
        &serde_json::json!({
            "path": "personas/trust-success.md", "old_str": "", "new_str": "# Once\n"
        }),
        "test",
    )
    .unwrap();
    let (first, second) = tokio::join!(
        execute_approved_draft(handle, &success_id),
        execute_approved_draft(handle, &success_id)
    );
    assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
    assert_eq!(
        draft_status(handle, &success_id).unwrap()["execution_status"],
        "succeeded"
    );

    // Failed claim persistence rolls the decision back, with no execution.
    store.conn.execute_batch("CREATE TRIGGER fail_claim BEFORE INSERT ON proactive_execution BEGIN SELECT RAISE(ABORT, 'injected claim failure'); END;").unwrap();
    let id = queue_tool_draft(handle, "test", "edit_file", &serde_json::json!({}), "test").unwrap();
    assert!(execute_approved_draft(handle, &id).await.is_err());
    assert!(draft_status(handle, &id).unwrap()["reviewed_at"].is_null());
    store
        .conn
        .execute_batch("DROP TRIGGER fail_claim;")
        .unwrap();

    // Side effect succeeds, but result storage fails. Reopening must not replay.
    let path = crate::self_files::resolve_allowed_path(handle, "personas/trust-test.md").unwrap();
    let id = queue_tool_draft(
        handle,
        "test",
        "edit_file",
        &serde_json::json!({
            "path": "personas/trust-test.md", "old_str": "", "new_str": "# Approved\n"
        }),
        "test",
    )
    .unwrap();
    store.conn.execute_batch("CREATE TRIGGER fail_result BEFORE UPDATE ON proactive_execution BEGIN SELECT RAISE(ABORT, 'injected result failure'); END;").unwrap();
    assert!(execute_approved_draft(handle, &id)
        .await
        .unwrap_err()
        .contains("Do not retry"));
    assert_eq!(std::fs::read_to_string(path).unwrap(), "# Approved\n");
    assert_eq!(
        draft_status(handle, &id).unwrap()["execution_status"],
        "unknown"
    );
    store
        .conn
        .execute_batch("DROP TRIGGER fail_result;")
        .unwrap();
    ensure_proactive_queue_table(handle).unwrap();
    assert!(execute_approved_draft(handle, &id).await.is_err());
    assert_eq!(
        draft_status(handle, &id).unwrap()["execution_status"],
        "unknown"
    );
    assert!(get_proactive_messages(handle, 100, Some("test"))
        .unwrap()
        .iter()
        .any(|m| m.id == id));

    // Legacy claimed rows retain their ambiguous decision and get unknown,
    // never fabricated success or rejection.
    let id = queue_tool_draft(handle, "test", "edit_file", &serde_json::json!({}), "test").unwrap();
    store
        .conn
        .execute(
            "UPDATE proactive_queue SET reviewed_at = 'legacy' WHERE id = ?1",
            [&id],
        )
        .unwrap();
    ensure_proactive_queue_table(handle).unwrap();
    let state = draft_status(handle, &id).unwrap();
    assert!(state["approved"].is_null());
    assert_eq!(state["execution_status"], "unknown");

    let scoped = queue_tool_draft(
        handle,
        "other-session",
        "edit_file",
        &serde_json::json!({}),
        "test",
    )
    .unwrap();
    let messages = get_proactive_messages(handle, 1, Some("other-session")).unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].id, scoped);
}

#[tokio::test]
async fn test_crystallize_sketch_draft_gated() {
    let _home_lock = home_lock_async().await;
    let _home_jail = HomeJail::new();
    let app = tauri::test::mock_app();
    let handle = app.handle();

    let path = crate::self_files::resolve_allowed_path(handle, "personas/test-sketch.md").unwrap();
    if path.exists() {
        let _ = std::fs::remove_file(&path);
    }

    let args = serde_json::json!({
        "logical_path": "personas/test-sketch.md",
        "markdown": "# Test Sketch Markdown\n",
        "source_sketch_id": "test-source-sketch-123"
    });

    let res = execute_draft_gated_tool(handle, "crystallize_sketch", &args).await;
    assert!(res.is_ok(), "Expected success, got error: {:?}", res.err());
    let msg = res.unwrap();
    assert!(msg.contains("Crystallized sketch"));
    assert!(msg.contains("test-source-sketch-123"));

    // Verify file content was written
    assert!(path.exists());
    let content = std::fs::read_to_string(&path).unwrap();
    assert_eq!(content, "# Test Sketch Markdown\n");

    // Clean up
    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn persona_approval_saves_only_reviewed_text_and_refuses_legacy_generation() {
    let _lock = home_lock_async().await;
    let _jail = HomeJail::new();
    let app = tauri::test::mock_app();
    let handle = app.handle();
    let path =
        crate::self_files::resolve_allowed_path(handle, "personas/reviewed-text.md").unwrap();
    let markdown = "# Personal travel\n\nAsk before booking.\n";
    let args = serde_json::json!({
        "logical_path": "personas/reviewed-text.md", "markdown": markdown,
        "source_sketch_id": "source"
    });
    let id = queue_tool_draft(
        handle,
        "chat",
        "crystallize_sketch",
        &args,
        "Review the exact text",
    )
    .unwrap();
    assert!(!path.exists(), "Queueing must not save the persona");
    let queued = get_proactive_messages(handle, 20, Some("chat")).unwrap();
    let payload: DraftPayload =
        serde_json::from_str(queued[0].draft_payload.as_ref().unwrap()).unwrap();
    assert_eq!(payload.arguments["markdown"], markdown);
    execute_approved_draft(handle, &id).await.unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), markdown);
    assert!(execute_approved_draft(handle, &id).await.is_err());

    let legacy = queue_tool_draft(
        handle,
        "chat",
        "crystallize_sketch",
        &serde_json::json!({"sketch_id":"source"}),
        "legacy",
    )
    .unwrap();
    assert!(execute_approved_draft(handle, &legacy)
        .await
        .unwrap_err()
        .contains("no text to review"));
    assert_eq!(
        draft_status(handle, &legacy).unwrap()["execution_status"],
        "failed"
    );

    let mut wrong_target = args.clone();
    wrong_target["logical_path"] = serde_json::json!("config.toml");
    assert!(
        execute_draft_gated_tool(handle, "crystallize_sketch", &wrong_target)
            .await
            .unwrap_err()
            .contains("persona file")
    );

    let rejected = queue_tool_draft(handle, "chat", "crystallize_sketch", &args, "reject").unwrap();
    review_proactive_message(handle, &rejected, Some(false)).unwrap();
    assert!(execute_approved_draft(handle, &rejected).await.is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), markdown);
}

#[tokio::test]
async fn attention_resurfaces_cross_session_failures_and_unfinished_plans() {
    let _lock = home_lock_async().await;
    let _jail = HomeJail::new();
    let app = tauri::test::mock_app();
    let handle = app.handle();
    let failed = queue_tool_draft(
        handle,
        "old-session",
        "edit_file",
        &serde_json::json!({}),
        "failure",
    )
    .unwrap();
    assert!(execute_approved_draft(handle, &failed).await.is_err());
    let store = crate::memories::get_vector_store(handle).unwrap();
    let unknown = queue_tool_draft(
        handle,
        "another-session",
        "edit_file",
        &serde_json::json!({}),
        "legacy",
    )
    .unwrap();
    store
        .conn
        .execute(
            "UPDATE proactive_queue SET reviewed_at = 'legacy' WHERE id = ?1",
            [&unknown],
        )
        .unwrap();
    let plans = crate::actions::plan(
        &store,
        "Compare apartments",
        &["Gather listings", "Compare rent"],
        Some("old-session"),
    )
    .unwrap();
    // A model marking the parent done does not prove its steps finished.
    crate::actions::complete(&store, &plans[0], None).unwrap();
    crate::actions::block(&store, &plans[1], "Listings unavailable").unwrap();
    let items = get_attention_items(handle).unwrap();
    assert_eq!(items.actions.len(), 2);
    assert!(items
        .actions
        .iter()
        .any(|m| m.id == failed && m.execution_status.as_deref() == Some("failed")));
    assert!(items
        .actions
        .iter()
        .any(|m| m.id == unknown && m.approved.is_none()));
    assert_eq!(items.plans[0].root_id, plans[0]);
    for step in &plans[1..] {
        crate::actions::update_status(
            &store,
            step,
            crate::actions::ActionStatus::Cancelled,
            None,
            None,
        )
        .unwrap();
    }
    assert!(get_attention_items(handle).unwrap().plans.is_empty());
}

#[tokio::test]
async fn persona_generation_happens_before_review_not_during_approval() {
    use crate::tests::agent_helpers::TestEnv;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, ResponseTemplate};
    let _lock = home_lock_async().await;
    let env = TestEnv::new().await;
    let handle = &env.handle;
    let store = crate::memories::get_vector_store(handle).unwrap();
    let ids =
        crate::actions::plan(&store, "Travel review", &["Compare trains"], Some("chat")).unwrap();
    crate::actions::complete(&store, &ids[1], None).unwrap();
    let config = crate::config::AppConfig {
        background_model: Some("gpt-oss-120b (Groq)".to_string()),
        groq_api_key: Some("test-only".to_string()),
        ..Default::default()
    };
    Mock::given(method("POST")).and(path("/groq/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices":[{"message":{"role":"assistant","content":"# Travel review\nAsk before booking."},"finish_reason":"stop"}]
        }))).expect(1).mount(&env.server).await;
    let id = prepare_tool_draft(
        handle,
        "chat",
        "crystallize_sketch",
        &serde_json::json!({"sketch_id":ids[0]}),
        "Review text",
        &config,
    )
    .await
    .unwrap();
    let rows = get_proactive_messages(handle, 20, Some("chat")).unwrap();
    let draft: DraftPayload =
        serde_json::from_str(rows[0].draft_payload.as_ref().unwrap()).unwrap();
    assert!(draft.arguments.get("sketch_id").is_none());
    let path = crate::self_files::resolve_allowed_path(
        handle,
        draft.arguments["logical_path"].as_str().unwrap(),
    )
    .unwrap();
    assert!(!path.exists());
    let reviewed = draft.arguments["markdown"].as_str().unwrap();
    assert!(reviewed.contains("Ask before booking."));
    execute_approved_draft(handle, &id).await.unwrap();
    assert_eq!(std::fs::read_to_string(path).unwrap(), reviewed);
    assert_eq!(env.server.received_requests().await.unwrap().len(), 1);
}
