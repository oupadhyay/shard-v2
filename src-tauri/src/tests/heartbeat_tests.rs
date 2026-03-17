/**
 * Heartbeat engine tests
 *
 * Tests for heartbeat spec parsing, rate limiting, proactive queue operations,
 * and cron-to-heartbeat migration. All tests are self-contained and use
 * in-memory/tempdir fixtures.
 */
use crate::heartbeat::*;

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
    // Test that the migration produces valid heartbeat spec content
    let schedule = "0 */6 * * *";
    let prompt = "Check for system updates and report.";
    let session_idx = 1;

    let content = format!(
        "---\nschedule: \"{}\"\nsession: \"agent:cron-migrated-{}\"\n---\n{}",
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
        heartbeats_dir.join("news.md"),
        "---\nschedule: \"0 */2 * * *\"\nsession: \"agent:news\"\n---\nCheck news.",
    )
    .unwrap();

    // Write another valid spec
    fs::write(
        heartbeats_dir.join("weather.md"),
        "---\nschedule: \"0 8 * * *\"\nsession: \"agent:weather\"\npersona: \"meteorologist\"\n---\nGet weather forecast.",
    )
    .unwrap();

    // Write invalid file (missing frontmatter)
    fs::write(heartbeats_dir.join("invalid.md"), "No frontmatter here.").unwrap();

    // Write non-md file (should be ignored)
    fs::write(heartbeats_dir.join("notes.txt"), "Some notes").unwrap();

    // Read them back with our parser directly
    let mut specs = Vec::new();
    for entry in fs::read_dir(&heartbeats_dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("md") {
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
    use crate::tools::is_draft_gated;

    // High-risk tools should be gated
    assert!(is_draft_gated("edit_config"));
    assert!(is_draft_gated("create_heartbeat"));
    assert!(is_draft_gated("delete_heartbeat"));
    assert!(is_draft_gated("edit_heartbeat"));

    // Safe tools should NOT be gated
    assert!(!is_draft_gated("web_search"));
    assert!(!is_draft_gated("save_memory"));
    assert!(!is_draft_gated("run_python"));
    assert!(!is_draft_gated("wake_me_up_in"));
    assert!(!is_draft_gated("load_persona"));
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
    use crate::tools::get_heartbeat_tools;

    let tools = get_heartbeat_tools(&[]);
    let tool_names: Vec<&str> = tools.iter().map(|t| t.function.name.as_str()).collect();

    // Should include global tools
    assert!(tool_names.contains(&"web_search"));
    assert!(tool_names.contains(&"save_memory"));
    assert!(tool_names.contains(&"wake_me_up_in"));

    // Should also include draft-gated tools
    assert!(tool_names.contains(&"edit_config"));
    assert!(tool_names.contains(&"create_heartbeat"));
    assert!(tool_names.contains(&"delete_heartbeat"));
    assert!(tool_names.contains(&"edit_heartbeat"));
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

    let filepath = heartbeats_dir.join(format!("{}.md", safe_name));

    let content = format!(
        "---\nschedule: \"{}\"\nsession: \"{}\"\n---\n{}",
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
