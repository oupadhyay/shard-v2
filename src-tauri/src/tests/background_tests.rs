/**
 * Background jobs tests
 *
 * Tests for cleanup and summary analysis background job functions.
 * LLM integration uses mocked responses to avoid API quota consumption.
 */

use crate::background::{
    analyze_interactions_in_dir, cleanup_interactions_in_dir, parse_cleanup_decision,
    parse_topic_updates, LOOKBACK_HOURS, LOG_RETENTION_DAYS,
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
    assert!(result.llm_reasoning.is_none(), "Fallback cleanup has no LLM reasoning");

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
    assert!(result.topics_updated.is_empty(), "Stats-only analysis has no topics");
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
