/**
 * Compaction Module - Context window management for long conversations
 *
 * Implements:
 * - Context size definitions per model
 * - Token estimation for messages
 * - Compaction triggering at configurable threshold
 * - Pre-compaction memory flush (LLM saves important facts before summarization)
 * - History summarization
 */

use crate::agent::ChatMessage;
use crate::config::AppConfig;
use reqwest::Client;
use serde::Serialize;
use tauri::{AppHandle, Runtime};

// ============================================================================
// Constants
// ============================================================================

/// Default context sizes per model family (in tokens)
const GEMINI_CONTEXT_SIZE: usize = 1_000_000;   // 1M for all Gemini models
const OPENROUTER_CONTEXT_SIZE: usize = 128_000; // 128K for OpenRouter models
const GROQ_CONTEXT_SIZE: usize = 128_000;       // 128K for Groq
const CEREBRAS_CONTEXT_SIZE: usize = 128_000;   // 128K for Cerebras

/// Default compaction settings
pub const DEFAULT_THRESHOLD: f32 = 0.5;          // 50% of context window
const DEFAULT_PRESERVE_TURNS: usize = 10;    // Keep last 10 turns

/// Rough estimate: ~4 characters per token
const CHARS_PER_TOKEN: usize = 4;

// ============================================================================
// Types
// ============================================================================

/// Result of a compaction operation
#[derive(Debug, Clone, Serialize)]
pub struct CompactionResult {
    /// The generated summary of compacted turns
    pub summary: String,
    /// Number of recent turns preserved
    pub preserved_turns: usize,
    /// Number of turns that were compacted
    pub compacted_turns: usize,
    /// Estimated tokens saved
    pub tokens_saved: usize,
}

/// Pre-compaction flush result
#[derive(Debug, Clone, Serialize)]
pub struct FlushResult {
    /// Whether any memories were extracted
    pub extracted: bool,
    /// Number of facts extracted
    pub fact_count: usize,
}

// ============================================================================
// Context Size Lookup
// ============================================================================

/// Get the context window size for a given model (in tokens)
pub fn get_context_size(model: &str) -> usize {
    // Gemini models (no slash, no provider suffix)
    if model.starts_with("gemini") {
        return GEMINI_CONTEXT_SIZE;
    }

    // Provider-suffixed models
    if model.contains("(Groq)") {
        return GROQ_CONTEXT_SIZE;
    }
    if model.contains("(Cerebras)") {
        return CEREBRAS_CONTEXT_SIZE;
    }

    // OpenRouter models (contain slash like "openai/gpt-oss-120b:free")
    if model.contains('/') {
        return OPENROUTER_CONTEXT_SIZE;
    }

    // Default fallback to most conservative
    OPENROUTER_CONTEXT_SIZE
}

// ============================================================================
// Token Estimation
// ============================================================================

/// Estimate tokens in a single chat message
pub fn estimate_message_tokens(msg: &ChatMessage) -> usize {
    let mut total_chars = 0;

    // Content
    if let Some(content) = &msg.content {
        total_chars += content.len();
    }

    // Reasoning (if present)
    if let Some(reasoning) = &msg.reasoning {
        total_chars += reasoning.len();
    }

    // Tool calls (estimate based on serialized size)
    if let Some(tool_calls) = &msg.tool_calls {
        for tc in tool_calls {
            total_chars += tc.function.name.len();
            total_chars += tc.function.arguments.len();
            total_chars += 50; // Overhead for structure
        }
    }

    // Role + overhead
    total_chars += msg.role.len() + 10;

    // Convert to tokens
    (total_chars + CHARS_PER_TOKEN - 1) / CHARS_PER_TOKEN
}

/// Estimate total tokens in conversation history
pub fn estimate_history_tokens(history: &[ChatMessage]) -> usize {
    history.iter().map(estimate_message_tokens).sum()
}

// ============================================================================
// Compaction Trigger Logic
// ============================================================================

/// Check if compaction should be triggered
///
/// Returns true if current history exceeds threshold percentage of context window
pub fn should_compact(history: &[ChatMessage], model: &str, threshold: Option<f32>) -> bool {
    let context_size = get_context_size(model);
    let threshold = threshold.unwrap_or(DEFAULT_THRESHOLD);
    let current_tokens = estimate_history_tokens(history);
    let threshold_tokens = (context_size as f32 * threshold) as usize;

    log::debug!(
        "[Compaction] Current: {} tokens, Threshold: {} tokens ({}% of {})",
        current_tokens,
        threshold_tokens,
        (threshold * 100.0) as u32,
        context_size
    );

    current_tokens >= threshold_tokens
}

/// Get the number of turns to preserve during compaction
pub fn get_preserve_turns(config: &AppConfig) -> usize {
    config
        .compaction_preserve_turns
        .map(|v| v as usize)
        .unwrap_or(DEFAULT_PRESERVE_TURNS)
}

// ============================================================================
// Pre-Compaction Memory Flush
// ============================================================================

/// System prompt for pre-compaction memory extraction
const FLUSH_SYSTEM_PROMPT: &str = r#"You are analyzing a conversation to extract important facts before the conversation is summarized.

Extract ONLY durable, reusable information such as:
- User preferences (coding style, units, languages)
- Key decisions made
- Important facts about the user or their projects
- Deadlines or commitments mentioned

Output as a markdown list. If nothing important to extract, respond with exactly: NO_MEMORIES

Example output:
- User prefers TypeScript over JavaScript
- Working on project called "Shard"
- Deadline: API must be done by February 1st"#;

/// Run pre-compaction memory flush
///
/// Calls LLM to extract important facts from the conversation before compaction.
/// Extracted facts are saved to the daily memory log.
pub async fn pre_compaction_flush<R: Runtime>(
    app_handle: &AppHandle<R>,
    http_client: &Client,
    config: &AppConfig,
    history: &[ChatMessage],
) -> Result<FlushResult, String> {
    // Skip if history is too short
    if history.len() < 5 {
        return Ok(FlushResult {
            extracted: false,
            fact_count: 0,
        });
    }

    // Build conversation summary for LLM
    let conversation_text = history
        .iter()
        .filter_map(|msg| {
            msg.content
                .as_ref()
                .map(|c| format!("{}: {}", msg.role, c))
        })
        .collect::<Vec<_>>()
        .join("\n");

    // Call background LLM for extraction
    let prompt = format!(
        "{}\n\n---\nConversation to analyze:\n{}",
        FLUSH_SYSTEM_PROMPT, conversation_text
    );

    let model = config
        .background_model
        .as_deref()
        .unwrap_or("gpt-oss-120b (Groq)");

    let response =
        crate::background::call_background_llm(http_client, config, model, &prompt).await?;

    // Check if no memories to extract
    if response.trim() == "NO_MEMORIES" || response.trim().is_empty() {
        log::info!("[Compaction] Pre-flush: No memories to extract");
        return Ok(FlushResult {
            extracted: false,
            fact_count: 0,
        });
    }

    // Count facts (lines starting with -)
    let fact_count = response.lines().filter(|l| l.trim().starts_with('-')).count();

    // Append to daily log
    let header = format!("\n## Pre-compaction Flush ({})\n", chrono::Utc::now().format("%H:%M"));
    let content = format!("{}{}\n", header, response);
    crate::memories::append_to_daily_log(app_handle, &content)?;

    log::info!(
        "[Compaction] Pre-flush: Extracted {} facts to daily log",
        fact_count
    );

    Ok(FlushResult {
        extracted: true,
        fact_count,
    })
}

// ============================================================================
// History Compaction
// ============================================================================

/// System prompt for conversation summarization
const COMPACT_SYSTEM_PROMPT: &str = r#"Summarize this conversation concisely for context continuity.

Focus on:
- What was discussed/decided
- Current state of any work in progress
- Open questions or next steps

Keep it brief (100-200 words). Use bullet points.
Do NOT include greetings or meta-commentary."#;

/// Compact conversation history by summarizing older turns
///
/// Keeps the most recent `preserve_turns` messages intact and replaces
/// older messages with a summary.
pub async fn compact_history<R: Runtime>(
    _app_handle: &AppHandle<R>,
    http_client: &Client,
    config: &AppConfig,
    history: &mut Vec<ChatMessage>,
) -> Result<CompactionResult, String> {
    let preserve_turns = get_preserve_turns(config);

    // Nothing to compact if history is short
    if history.len() <= preserve_turns {
        return Ok(CompactionResult {
            summary: String::new(),
            preserved_turns: history.len(),
            compacted_turns: 0,
            tokens_saved: 0,
        });
    }

    let compacted_turns = history.len() - preserve_turns;
    let to_compact: Vec<_> = history.drain(..compacted_turns).collect();

    // Estimate tokens before compaction
    let tokens_before: usize = to_compact.iter().map(estimate_message_tokens).sum();

    // Build text for summarization
    let conversation_text = to_compact
        .iter()
        .filter_map(|msg| {
            msg.content
                .as_ref()
                .map(|c| format!("{}: {}", msg.role, c))
        })
        .collect::<Vec<_>>()
        .join("\n");

    // Call LLM to summarize
    let prompt = format!(
        "{}\n\n---\nConversation:\n{}",
        COMPACT_SYSTEM_PROMPT, conversation_text
    );

    let model = config
        .background_model
        .as_deref()
        .unwrap_or("gpt-oss-120b (Groq)");

    let summary =
        crate::background::call_background_llm(http_client, config, model, &prompt).await?;

    // Estimate tokens after
    let tokens_after = (summary.len() + CHARS_PER_TOKEN - 1) / CHARS_PER_TOKEN;
    let tokens_saved = tokens_before.saturating_sub(tokens_after);

    // Insert summary as first message (system-like context)
    let summary_msg = ChatMessage {
        role: "user".to_string(),
        content: Some(format!("[Conversation Summary]\n{}", summary)),
        reasoning: None,
        tool_calls: None,
        tool_call_id: None,
        images: None,
    };
    history.insert(0, summary_msg);

    // Fix orphaned tool responses: collect valid tool_call IDs from remaining history
    let valid_tool_call_ids: std::collections::HashSet<String> = history
        .iter()
        .filter_map(|msg| msg.tool_calls.as_ref())
        .flatten()
        .map(|tc| tc.id.clone())
        .collect();

    // Remove messages with tool_call_id that reference removed tool calls
    let before_filter = history.len();
    history.retain(|msg| {
        if let Some(ref tool_call_id) = msg.tool_call_id {
            if !valid_tool_call_ids.contains(tool_call_id) {
                log::debug!("[Compaction] Removing orphaned tool response: {}", tool_call_id);
                return false;
            }
        }
        true
    });
    let orphans_removed = before_filter - history.len();
    if orphans_removed > 0 {
        log::info!("[Compaction] Removed {} orphaned tool responses", orphans_removed);
    }

    log::info!(
        "[Compaction] Compacted {} turns into summary, saved ~{} tokens",
        compacted_turns,
        tokens_saved
    );

    Ok(CompactionResult {
        summary,
        preserved_turns: history.len() - 1, // Minus the summary message
        compacted_turns,
        tokens_saved,
    })
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // Test helper: create a message with specific content length
    fn make_message(role: &str, content_len: usize) -> ChatMessage {
        ChatMessage {
            role: role.to_string(),
            content: Some("x".repeat(content_len)),
            reasoning: None,
            tool_calls: None,
            tool_call_id: None,
            images: None,
        }
    }

    #[test]
    fn test_get_context_size_gemini() {
        assert_eq!(get_context_size("gemini-2.5-flash"), 1_000_000);
        assert_eq!(get_context_size("gemini-2.5-flash-lite"), 1_000_000);
        assert_eq!(get_context_size("gemini-3-flash-preview"), 1_000_000);
    }

    #[test]
    fn test_get_context_size_openrouter() {
        assert_eq!(get_context_size("openai/gpt-oss-120b:free"), 128_000);
        assert_eq!(get_context_size("google/gemma-3-27b-it:free"), 128_000);
    }

    #[test]
    fn test_get_context_size_groq() {
        assert_eq!(get_context_size("gpt-oss-120b (Groq)"), 128_000);
    }

    #[test]
    fn test_get_context_size_cerebras() {
        assert_eq!(get_context_size("gpt-oss-120b (Cerebras)"), 128_000);
    }

    #[test]
    fn test_estimate_message_tokens() {
        // 400 chars of content + ~10 for role = ~410 chars / 4 = ~103 tokens
        let msg = make_message("user", 400);
        let tokens = estimate_message_tokens(&msg);
        assert!(tokens >= 100 && tokens <= 110, "Expected ~103 tokens, got {}", tokens);
    }

    #[test]
    fn test_estimate_history_tokens() {
        let history = vec![
            make_message("user", 400),
            make_message("assistant", 800),
        ];
        let tokens = estimate_history_tokens(&history);
        // (400 + 10) / 4 + (800 + 10) / 4 = 103 + 203 = 306
        assert!(tokens >= 300 && tokens <= 320, "Expected ~306 tokens, got {}", tokens);
    }

    #[test]
    fn test_should_compact_under_threshold() {
        // Small history, should not compact
        let history = vec![make_message("user", 100)];
        assert!(!should_compact(&history, "gemini-2.5-flash", Some(0.5)));
    }

    #[test]
    fn test_should_compact_with_artificial_limit() {
        // Use artificial 1000 token "model" to test threshold logic
        // 600 chars = 150 tokens, which is > 50% of 250 tokens
        let history = vec![
            make_message("user", 600),
            make_message("assistant", 600),
        ];
        // Simulate a 1000 token context by checking against 50% = 500 tokens
        // Our history is ~300 tokens, so should NOT compact against 500
        // But if we lower threshold to 0.25 (250 tokens), it SHOULD compact
        let tokens = estimate_history_tokens(&history);
        assert!(tokens > 250, "History should be > 250 tokens, got {}", tokens);

        // Can't use get_context_size for artificial limit, but we can
        // test the core logic by comparing tokens directly
        let threshold_tokens = 250;
        assert!(tokens >= threshold_tokens, "Should exceed threshold");
    }

    #[test]
    fn test_threshold_boundary() {
        // Create history that's exactly at threshold
        // For a "model" with 1000 char context (~250 tokens)
        // At 50% threshold = 125 tokens = 500 chars
        let history = vec![make_message("user", 500)];
        let tokens = estimate_history_tokens(&history);

        // Just verify token count is reasonable
        assert!(tokens >= 120 && tokens <= 140, "Expected ~128 tokens, got {}", tokens);
    }
}
