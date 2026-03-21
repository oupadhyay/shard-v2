/**
 * Background jobs tests
 *
 * Tests for cleanup and summary analysis background job functions.
 * LLM integration uses mocked responses to avoid API quota consumption.
 */
use crate::background::{
    analyze_interactions_in_dir, cleanup_interactions_in_dir, parse_cleanup_decision,
    parse_deriver_response, parse_dream_response, parse_rate_limit_wait,
    parse_topic_updates, LOG_RETENTION_DAYS, LOOKBACK_HOURS,
};
use chrono::{Duration as ChronoDuration, Utc};
use std::fs;
use std::io::Write;
use tempfile::TempDir;

#[test]
fn test_date_comparison() {
    let older = "2024-01-01";
    let newer = "2024-12-08";
    assert!(older < newer);
}

#[test]
fn test_retention_days() {
    assert_eq!(LOG_RETENTION_DAYS, 30);
}

#[test]
fn test_lookback_hours() {
    assert_eq!(LOOKBACK_HOURS, 12);
}

/// Create a dummy interaction JSONL file
fn create_interaction_file(dir: &std::path::Path, date: &str, entries: &[(&str, &str)]) {
    let filename = format!("interactions-{}.jsonl", date);
    let path = dir.join(filename);
    let mut file = fs::File::create(&path).expect("Failed to create test file");

    for (role, content) in entries {
        let entry = serde_json::json!({
            "ts": format!("{}T12:00:00Z", date),
            "role": role,
            "content": content
        });
        writeln!(file, "{}", entry).expect("Failed to write entry");
    }
}

#[test]
fn test_cleanup_removes_old_files() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let interactions_dir = temp_dir.path().join("interactions");
    fs::create_dir_all(&interactions_dir).expect("Failed to create interactions dir");

    // Create old file (60 days ago - should be deleted)
    let old_date = (Utc::now() - ChronoDuration::days(60))
        .format("%Y-%m-%d")
        .to_string();
    create_interaction_file(
        &interactions_dir,
        &old_date,
        &[("user", "Old message"), ("assistant", "Old response")],
    );

    // Create recent file (5 days ago - should be kept)
    let recent_date = (Utc::now() - ChronoDuration::days(5))
        .format("%Y-%m-%d")
        .to_string();
    create_interaction_file(
        &interactions_dir,
        &recent_date,
        &[("user", "Recent message")],
    );

    // Run cleanup with 30 day retention
    let result = cleanup_interactions_in_dir(&interactions_dir, 30).expect("Cleanup failed");

    assert_eq!(result.deleted_count, 1, "Should delete 1 old file");
    assert!(result.bytes_freed > 0, "Should have freed some bytes");
    assert!(
        result.llm_reasoning.is_none(),
        "Fallback cleanup has no LLM reasoning"
    );

    // Verify old file is gone, recent file remains
    let old_path = interactions_dir.join(format!("interactions-{}.jsonl", old_date));
    let recent_path = interactions_dir.join(format!("interactions-{}.jsonl", recent_date));

    assert!(!old_path.exists(), "Old file should be deleted");
    assert!(recent_path.exists(), "Recent file should remain");
}

#[test]
fn test_cleanup_ignores_non_jsonl_files() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let interactions_dir = temp_dir.path().join("interactions");
    fs::create_dir_all(&interactions_dir).expect("Failed to create interactions dir");

    // Create an old .txt file (should NOT be deleted)
    let old_date = (Utc::now() - ChronoDuration::days(60))
        .format("%Y-%m-%d")
        .to_string();
    let txt_path = interactions_dir.join(format!("interactions-{}.txt", old_date));
    fs::write(&txt_path, "Some text").expect("Failed to write txt file");

    let result = cleanup_interactions_in_dir(&interactions_dir, 30).expect("Cleanup failed");

    assert_eq!(result.deleted_count, 0, "Should not delete .txt files");
    assert!(txt_path.exists(), ".txt file should remain");
}

#[test]
fn test_cleanup_on_nonexistent_dir() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let nonexistent = temp_dir.path().join("does_not_exist");

    let result = cleanup_interactions_in_dir(&nonexistent, 30).expect("Should not error");

    assert_eq!(result.deleted_count, 0);
    assert_eq!(result.bytes_freed, 0);
}

#[test]
fn test_analyze_counts_messages() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let interactions_dir = temp_dir.path().join("interactions");
    fs::create_dir_all(&interactions_dir).expect("Failed to create interactions dir");

    // Create today's file with mixed messages
    let today = Utc::now().format("%Y-%m-%d").to_string();
    create_interaction_file(
        &interactions_dir,
        &today,
        &[
            ("user", "Hello"),
            ("assistant", "Hi there!"),
            ("user", "How are you?"),
            ("model", "I'm doing well"),
            ("user", "Great!"),
        ],
    );

    let result = analyze_interactions_in_dir(&interactions_dir, 24).expect("Analysis failed");

    assert_eq!(result.total_interactions, 5);
    assert_eq!(result.user_messages, 3);
    assert_eq!(result.assistant_messages, 2); // "assistant" + "model"
    assert!(result.total_chars > 0);
    assert!(
        result.topics_updated.is_empty(),
        "Stats-only analysis has no topics"
    );
}

#[test]
fn test_analyze_ignores_old_files() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let interactions_dir = temp_dir.path().join("interactions");
    fs::create_dir_all(&interactions_dir).expect("Failed to create interactions dir");

    // Create old file (5 days ago - outside 24h window)
    let old_date = (Utc::now() - ChronoDuration::days(5))
        .format("%Y-%m-%d")
        .to_string();
    create_interaction_file(
        &interactions_dir,
        &old_date,
        &[("user", "Old message"), ("assistant", "Old response")],
    );

    // Create today's file
    let today = Utc::now().format("%Y-%m-%d").to_string();
    create_interaction_file(&interactions_dir, &today, &[("user", "Today's message")]);

    let result = analyze_interactions_in_dir(&interactions_dir, 24).expect("Analysis failed");

    // Should only count today's message (old file is outside 24h window)
    assert_eq!(result.total_interactions, 1);
    assert_eq!(result.user_messages, 1);
}

#[test]
fn test_analyze_on_nonexistent_dir() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let nonexistent = temp_dir.path().join("does_not_exist");

    let result = analyze_interactions_in_dir(&nonexistent, 24).expect("Should not error");

    assert_eq!(result.total_interactions, 0);
    assert_eq!(result.user_messages, 0);
    assert_eq!(result.assistant_messages, 0);
    assert_eq!(result.total_chars, 0);
}

#[test]
fn test_analyze_calculates_char_count() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let interactions_dir = temp_dir.path().join("interactions");
    fs::create_dir_all(&interactions_dir).expect("Failed to create interactions dir");

    let today = Utc::now().format("%Y-%m-%d").to_string();
    create_interaction_file(
        &interactions_dir,
        &today,
        &[
            ("user", "12345"),      // 5 chars
            ("assistant", "67890"), // 5 chars
        ],
    );

    let result = analyze_interactions_in_dir(&interactions_dir, 24).expect("Analysis failed");

    assert_eq!(result.total_chars, 10);
}

// ============================================================================
// LLM Response Parsing Tests (Mocked)
// ============================================================================

#[test]
fn test_parse_topic_updates_valid_json() {
    let llm_response = r#"
Here are the extracted topics:

[
  {"topic": "SHARD", "summary": "Working on Shard v2, a Tauri-based AI assistant."},
  {"topic": "Rust", "summary": "User prefers Rust for backend development."}
]

These are the key insights from the interactions.
"#;

    let result = parse_topic_updates(llm_response).expect("Should parse successfully");

    assert_eq!(result.len(), 2);
    assert_eq!(result[0].topic, "SHARD");
    assert!(result[0].summary.contains("Tauri"));
    assert_eq!(result[1].topic, "Rust");
}

#[test]
fn test_parse_topic_updates_empty_array() {
    let llm_response = "No significant topics found. []";

    let result = parse_topic_updates(llm_response).expect("Should parse successfully");

    assert!(result.is_empty());
}

#[test]
fn test_parse_topic_updates_no_json() {
    let llm_response = "I couldn't find any topics in the interactions.";

    let result = parse_topic_updates(llm_response);

    assert!(result.is_err());
}

#[test]
fn test_parse_cleanup_decision_valid_json() {
    let llm_response = r#"
Based on the analysis:

{
  "to_remove": ["2024-12-10T10:00:00Z", "2024-12-10T11:30:00Z"],
  "reasoning": "These are generic greetings that add no context."
}

The remaining entries should be kept.
"#;

    let result = parse_cleanup_decision(llm_response).expect("Should parse successfully");

    assert_eq!(result.to_remove.len(), 2);
    assert!(result.to_remove[0].contains("10:00:00"));
    assert!(result.reasoning.contains("greetings"));
}

#[test]
fn test_parse_cleanup_decision_empty_removal() {
    let llm_response = r#"{"to_remove": [], "reasoning": "All entries contain valuable context."}"#;

    let result = parse_cleanup_decision(llm_response).expect("Should parse successfully");

    assert!(result.to_remove.is_empty());
    assert!(result.reasoning.contains("valuable"));
}

#[test]
fn test_parse_cleanup_decision_no_json() {
    let llm_response = "I recommend keeping all entries.";

    let result = parse_cleanup_decision(llm_response);

    assert!(result.is_err());
}

#[test]
fn test_cleanup_ignores_sessions() {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let sessions_dir = temp_dir.path().join("sessions");
    fs::create_dir_all(&sessions_dir).expect("Failed to create sessions dir");

    // Create an old session file
    let old_date = (Utc::now() - ChronoDuration::days(60))
        .format("%Y-%m-%d")
        .to_string();
    let session_path = sessions_dir.join(format!("{}-old-slug.md", old_date));
    fs::write(&session_path, "Session content").expect("Failed to write session file");

    let result = cleanup_interactions_in_dir(&sessions_dir, 30).expect("Cleanup failed");

    // Cleanup interactions only targets .jsonl files, so .md files should be ignored completely
    assert_eq!(result.deleted_count, 0, "Should not delete .md files");
    assert!(session_path.exists(), "Session file should remain untouched");
}

// ============================================================================
// Deriver Pipeline Tests
// ============================================================================

#[test]
fn test_parse_deriver_response_valid() {
    let response = r#"Here are the facts: {"facts": [{"fact": "User prefers Rust"}, {"fact": "User lives in SF"}]}"#;
    let facts = parse_deriver_response(response);
    assert_eq!(facts.len(), 2);
    assert_eq!(facts[0].fact, "User prefers Rust");
    assert_eq!(facts[1].fact, "User lives in SF");
}

#[test]
fn test_parse_deriver_response_with_session() {
    let response = r#"{"facts": [{"fact": "User has a cat", "session": "session-123"}]}"#;
    let facts = parse_deriver_response(response);
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].session.as_deref(), Some("session-123"));
}

#[test]
fn test_parse_deriver_response_empty_facts() {
    let response = r#"{"facts": []}"#;
    let facts = parse_deriver_response(response);
    assert!(facts.is_empty());
}

#[test]
fn test_parse_deriver_response_no_json() {
    let response = "I couldn't find any facts about the user.";
    let facts = parse_deriver_response(response);
    assert!(facts.is_empty());
}

#[test]
fn test_parse_deriver_response_wrapped_in_markdown() {
    let response = "```json\n{\"facts\": [{\"fact\": \"User uses Tauri\"}]}\n```";
    let facts = parse_deriver_response(response);
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].fact, "User uses Tauri");
}

// ============================================================================
// Dream Phase Tests
// ============================================================================

#[test]
fn test_parse_dream_response_valid() {
    let response = r#"{"observations": [{"content": "User favors typed languages", "source_ids": ["id1", "id2"], "level": "inductive"}], "peer_card_facts": ["Software engineer", "Lives in SF"]}"#;
    let dream = parse_dream_response(response);
    assert_eq!(dream.observations.len(), 1);
    assert_eq!(dream.observations[0].content, "User favors typed languages");
    assert_eq!(dream.observations[0].level, "inductive");
    assert_eq!(dream.observations[0].source_ids, vec!["id1", "id2"]);
    assert_eq!(dream.peer_card_facts.len(), 2);
}

#[test]
fn test_parse_dream_response_empty() {
    let response = r#"{"observations": [], "peer_card_facts": []}"#;
    let dream = parse_dream_response(response);
    assert!(dream.observations.is_empty());
    assert!(dream.peer_card_facts.is_empty());
}

#[test]
fn test_parse_dream_response_no_peer_card() {
    let response = r#"{"observations": [{"content": "Test", "source_ids": [], "level": "deductive"}]}"#;
    let dream = parse_dream_response(response);
    assert_eq!(dream.observations.len(), 1);
    assert!(dream.peer_card_facts.is_empty()); // serde default
}

#[test]
fn test_parse_dream_response_no_json() {
    let response = "Nothing to derive here.";
    let dream = parse_dream_response(response);
    assert!(dream.observations.is_empty());
    assert!(dream.peer_card_facts.is_empty());
}

#[test]
fn test_parse_dream_response_with_contradictions() {
    let response = r#"{"observations": [
        {"content": "User lives in both SF and NYC", "source_ids": ["a", "b"], "level": "contradiction"},
        {"content": "User commutes daily", "source_ids": ["c"], "level": "deductive"}
    ], "peer_card_facts": []}"#;
    let dream = parse_dream_response(response);
    assert_eq!(dream.observations.len(), 2);
    assert_eq!(dream.observations[0].level, "contradiction");
    assert_eq!(dream.observations[1].level, "deductive");
}

// ============================================================================
// Rate Limit Parser Tests
// ============================================================================

#[test]
fn test_parse_rate_limit_seconds() {
    let error = r#"Background LLM API error: {"error":{"message":"Rate limit reached for model `openai/gpt-oss-20b` ... Please try again in 18.48s.","type":"tokens","code":"rate_limit_exceeded"}}"#;
    let wait = parse_rate_limit_wait(error).unwrap();
    assert!((wait - 19.48).abs() < 0.1); // 18.48 + 1.0 buffer
}

#[test]
fn test_parse_rate_limit_minutes_and_seconds() {
    let error = r#"{"error":{"message":"Rate limit exceeded. Please try again in 1m30s","code":"rate_limit_exceeded"}}"#;
    let wait = parse_rate_limit_wait(error).unwrap();
    assert!((wait - 91.0).abs() < 0.1); // 60 + 30 + 1.0 buffer
}

#[test]
fn test_parse_rate_limit_minutes_only() {
    let error = r#"{"error":{"message":"Rate limit. Please try again in 2m","code":"rate_limit_exceeded"}}"#;
    let wait = parse_rate_limit_wait(error).unwrap();
    assert!((wait - 121.0).abs() < 0.1); // 120 + 1.0 buffer
}

#[test]
fn test_parse_rate_limit_generic() {
    let error = r#"{"error":{"message":"Rate limit exceeded","code":"rate_limit_exceeded"}}"#;
    let wait = parse_rate_limit_wait(error).unwrap();
    assert_eq!(wait, 30.0); // default fallback
}

#[test]
fn test_parse_rate_limit_not_rate_limited() {
    let error = "Background LLM API error: Internal server error";
    assert!(parse_rate_limit_wait(error).is_none());
}

#[test]
fn test_parse_rate_limit_daily_not_retried() {
    let error = r#"{"error":{"message":"Rate limit: 100 requests per day exceeded","code":"rate_limit_exceeded"}}"#;
    assert!(parse_rate_limit_wait(error).is_none());
}

#[test]
fn test_parse_rate_limit_http_429() {
    let error = "HTTP 429 Too Many Requests. Please try again in 5s.";
    let wait = parse_rate_limit_wait(error).unwrap();
    assert!((wait - 6.0).abs() < 0.1); // 5 + 1.0 buffer
}

#[test]
fn test_parse_rate_limit_real_groq_error() {
    let error = r#"Background LLM API error: {"error":{"message":"Rate limit reached for model `openai/gpt-oss-20b` in organization `org_01kcgcb5zwf40vq0ck1ghb6wcv` service tier `on_demand` on tokens per minute (TPM): Limit 8000, Used 7980, Requested 2484. Please try again in 18.48s. Need more tokens? Upgrade to Dev Tier today at https://console.groq.com/settings/billing","type":"tokens","code":"rate_limit_exceeded"}}"#;
    let wait = parse_rate_limit_wait(error).unwrap();
    assert!((wait - 19.48).abs() < 0.1);
}

#[test]
fn test_parse_rate_limit_real_groq_error_13s() {
    let error = r#"Background LLM API error: {"error":{"message":"Rate limit reached for model `openai/gpt-oss-20b` in organization `org_01kcgcb5zwf40vq0ck1ghb6wcv` service tier `on_demand` on tokens per minute (TPM): Limit 8000, Used 7961, Requested 1873. Please try again in 13.755s. Need more tokens? Upgrade to Dev Tier today at https://console.groq.com/settings/billing","type":"tokens","code":"rate_limit_exceeded"}}"#;
    let wait = parse_rate_limit_wait(error).unwrap();
    assert!((wait - 14.755).abs() < 0.1); // 13.755 + 1.0 buffer
}
