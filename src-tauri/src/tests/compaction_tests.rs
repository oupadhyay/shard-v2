use crate::agent::ChatMessage;
/**
 * Compaction system tests
 *
 * Tests for context window management and history compaction.
 */
use crate::compaction::{
    estimate_history_tokens, estimate_message_tokens, get_context_size, should_compact,
};

// Test helper: create a message with specific content length
fn make_message(role: &str, content_len: usize) -> ChatMessage {
    ChatMessage {
        role: role.to_string(),
        content: Some("x".repeat(content_len)),
        reasoning: None,
        tool_calls: None,
        tool_call_id: None,
        is_cron: None,
        images: None,
    }
}

#[test]
fn test_get_context_size_gemini() {
    assert_eq!(get_context_size("gemini-2.5-flash"), 1_000_000);
    assert_eq!(get_context_size("gemini-3.1-flash-lite-preview"), 1_000_000);
    assert_eq!(get_context_size("gemini-3-flash-preview"), 1_000_000);
}

#[test]
fn test_get_context_size_openrouter() {
    assert_eq!(get_context_size("openai/gpt-oss-120b:free"), 128_000);
    assert_eq!(get_context_size("google/gemma-4-31b-it:free"), 128_000);
}

#[test]
fn test_get_context_size_groq() {
    assert_eq!(get_context_size("gpt-oss-120b (Groq)"), 128_000);
}

#[test]
fn test_estimate_message_tokens() {
    // 400 chars of content + ~10 for role = ~410 chars / 4 = ~103 tokens
    let msg = make_message("user", 400);
    let tokens = estimate_message_tokens(&msg);
    assert!(
        (100..=110).contains(&tokens),
        "Expected ~103 tokens, got {}",
        tokens
    );
}

#[test]
fn test_estimate_history_tokens() {
    let history = vec![make_message("user", 400), make_message("assistant", 800)];
    let tokens = estimate_history_tokens(&history);
    // (400 + 10) / 4 + (800 + 10) / 4 = 103 + 203 = 306
    assert!(
        (300..=320).contains(&tokens),
        "Expected ~306 tokens, got {}",
        tokens
    );
}

#[test]
fn test_should_compact_under_threshold() {
    // Small history, should not compact
    let history = vec![make_message("user", 100)];
    assert!(!should_compact(&history, "gemini-2.5-flash", Some(0.5)));
}

#[test]
fn test_should_compact_with_artificial_limit() {
    // Create history that would exceed 50% of a small context
    // For a model with 1000 token context, 50% = 500 tokens
    // 2000 chars = ~500 tokens, so this should trigger compaction
    // at a very low threshold

    let history = vec![make_message("user", 2000), make_message("assistant", 2000)];
    let tokens = estimate_history_tokens(&history);

    // With ~1000 tokens, this would trigger at 50% for a 2000 token model
    // Since we can't inject artificial limits directly, we test the inverse:
    // This history should NOT trigger compaction for Gemini (1M context)
    assert!(!should_compact(&history, "gemini-2.5-flash", Some(0.5)));

    // But we can test that token estimation is working correctly
    assert!(
        (950..=1100).contains(&tokens),
        "Expected ~1000 tokens, got {}",
        tokens
    );
}

#[test]
fn test_threshold_boundary() {
    // Test that threshold parameter works correctly
    let history = vec![make_message("user", 4000)]; // ~1000 tokens

    // At 0.1% threshold for Gemini (1M * 0.001 = 1000 tokens), should trigger
    assert!(should_compact(&history, "gemini-2.5-flash", Some(0.001)));

    // At 1% threshold (10K tokens), should NOT trigger
    assert!(!should_compact(&history, "gemini-2.5-flash", Some(0.01)));
}

#[test]
fn test_message_with_reasoning() {
    // Verify reasoning content is counted in token estimation
    let mut msg = make_message("assistant", 400);
    msg.reasoning = Some("This is my reasoning about 100 chars long...".to_string());

    let tokens = estimate_message_tokens(&msg);
    // Should be more than a message without reasoning
    assert!(tokens > 103, "Reasoning should add to token count");
}
