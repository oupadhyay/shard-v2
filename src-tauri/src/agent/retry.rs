//! Frontend-triggered retry plumbing.
//!
//! When the renderer detects a KaTeX parse error in the latest assistant
//! message, it invokes [`Agent::retry_with_katex_hint`]. The handler pops the
//! failed message, injects a hint user message, emits an `agent-retry` event,
//! and runs a single fresh turn against the chosen provider.
//!
//! This module does NOT cover the *backend*-side empty-response retry — that
//! lives in the `process_message` loop in `agent/mod.rs` (the loop counter
//! and gating are tightly coupled to the message-handling state machine).

use tauri::{AppHandle, Emitter};

use super::types::{ChatMessage, RetryReason};
use super::{Agent, TurnContext};

impl<R: tauri::Runtime> Agent<R> {
    /// Retry the last response with a hint about KaTeX errors.
    /// Called by the frontend when KaTeX parsing fails.
    pub async fn retry_with_katex_hint(
        &self,
        app_handle: &AppHandle<R>,
        katex_errors: Vec<String>,
        config: &crate::config::AppConfig,
    ) -> Result<(), String> {
        let mut history = self.history.lock().await;

        // Check if retry on KaTeX is enabled
        if !config.retry_on_katex.unwrap_or(true) {
            return Ok(());
        }

        // Find and remove the last assistant message
        if let Some(last_msg) = history.last() {
            if last_msg.role == "assistant" || last_msg.role == "model" {
                history.pop();

                // Add the retry hint
                let hint = RetryReason::MalformedLatex {
                    errors: katex_errors,
                }
                .get_hint();
                let msg = ChatMessage {
                    role: "user".to_string(),
                    content: Some(hint),
                    reasoning: None,
                    tool_calls: None,
                    tool_call_id: None,
                    is_cron: None,
                    images: None,
                };
                history.push(msg.clone());
                self.insert_single_message_to_db(app_handle, &msg).await;

                // Emit retry event
                let retry_event = serde_json::json!({
                    "reason": "katex_error",
                    "attempt": 1,
                    "max": config.max_auto_retries.unwrap_or(2)
                });
                app_handle.emit("agent-retry", retry_event.to_string()).ok();

                // Release lock and run another turn
                drop(history);

                // Re-process with the hint by calling the internal method below.
                self.run_retry_turn(app_handle, config).await?;
            }
        }

        Ok(())
    }

    /// Internal method to run a retry turn after hint injection.
    async fn run_retry_turn(
        &self,
        app_handle: &AppHandle<R>,
        config: &crate::config::AppConfig,
    ) -> Result<(), String> {
        let mut history = self.history.lock().await;

        let stream_id =
            crate::CURRENT_STREAM_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;

        let selected_model = config
            .selected_model
            .clone()
            .unwrap_or("gemini-3.1-flash-lite-preview".to_string());

        let is_gemini = crate::models::is_gemini_model(&selected_model);

        let _continue_turn = if is_gemini {
            self.process_gemini_turn(
                app_handle,
                config,
                &mut history,
                stream_id,
                &TurnContext::default(),
            )
            .await?
        } else {
            self.process_openrouter_turn(
                app_handle,
                config,
                &mut history,
                stream_id,
                &TurnContext::default(),
            )
            .await?
        };

        // Persist the new response
        drop(history);
        self.persist_history().await;

        Ok(())
    }
}
