//! Content-addressed history hashing.
//!
//! Used to detect whether the chat history has changed since the last
//! archive, so we can skip redundant LLM-driven session summarization.

use super::types::ChatMessage;

/// Compute a stable, content-addressed hash of the chat history.
///
/// Hashes only the fields that materially affect a transcript: role,
/// content, and the id/name/arguments of every tool call. Reasoning,
/// images, and `is_cron` are intentionally excluded because they don't
/// change archive value.
pub(crate) fn calculate_history_hash(history: &[ChatMessage]) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    for msg in history {
        msg.role.hash(&mut hasher);
        if let Some(content) = &msg.content {
            content.hash(&mut hasher);
        }
        if let Some(calls) = &msg.tool_calls {
            for call in calls {
                call.id.hash(&mut hasher);
                call.function.name.hash(&mut hasher);
                call.function.arguments.hash(&mut hasher);
            }
        }
    }
    hasher.finish()
}
