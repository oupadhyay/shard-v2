/**
 * Agent module - AI chat agent with Gemini and OpenRouter support
 */
mod gemini;
pub(crate) mod openrouter;
mod types;

pub use gemini::{construct_gemini_messages, parse_gemini_chunk, AgentEvent};
pub use openrouter::{has_images, supports_tools, to_multimodal_messages};
pub use types::*;

use crate::integrations::{
    arxiv::{perform_arxiv_lookup, read_arxiv_paper},
    finance::perform_finance_lookup,
    weather::perform_weather_lookup,
    web_search::perform_web_search,
    wikipedia::perform_wikipedia_lookup,
};
use reqwest::Client;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tokio::sync::Mutex;

/// The main AI Agent managing chat history and API interactions
pub struct Agent {
    history: Mutex<Vec<ChatMessage>>,
    http_client: Client,
    uploaded_files: Mutex<Vec<String>>,
    backup_history: Mutex<Option<(Vec<ChatMessage>, String)>>,
    pub session_id: Mutex<String>,
    pub last_archived_hash: Mutex<u64>,
    pub app_handle: tauri::AppHandle,
}

impl Agent {
    pub fn new(app_handle: tauri::AppHandle) -> Self {
        let app_data_dir = app_handle
            .path()
            .app_data_dir()
            .expect("failed to get app data dir");
        std::fs::create_dir_all(&app_data_dir).expect("failed to create app data dir");

        let http_client = Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .unwrap_or_else(|_| Client::new());

        // Restore active session history from SQLite
        let mut session_id = uuid::Uuid::new_v4().to_string();
        let last_archived_hash = 0;
        let mut history = Vec::new();

        if let Ok(store) = crate::memories::get_vector_store(&app_handle) {
            use rusqlite::OptionalExtension;
            // Fetch the most recently updated session ID
            if let Ok(Some(latest_session_id)) = store
                .conn
                .query_row(
                    "SELECT id FROM sessions ORDER BY updated_at DESC LIMIT 1",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .optional()
            {
                session_id = latest_session_id;
                // Fetch messages for this session
                if let Ok(mut stmt) = store.conn.prepare(
                    "SELECT content FROM messages WHERE session_id = ? ORDER BY created_at ASC",
                ) {
                    if let Ok(msg_iter) = stmt.query_map([&session_id], |row| {
                        let content: String = row.get(0)?;
                        Ok(serde_json::from_str::<PersistedChatState>(&content)
                            .map(|s| s.history)
                            .unwrap_or_else(|_| {
                                if let Ok(m) = serde_json::from_str::<ChatMessage>(&content) {
                                    vec![m]
                                } else {
                                    Vec::new()
                                }
                            }))
                    }) {
                        for msg_res in msg_iter.flatten() {
                            history.extend(msg_res);
                        }
                    }
                }

                // Dynamic fallback for legacy markdown migrations
                if history.len() == 1 {
                    let first_msg = &history[0];
                    if first_msg.role == "assistant"
                        && first_msg
                            .content
                            .as_deref()
                            .unwrap_or("")
                            .starts_with("# Session Transcript")
                    {
                        let parsed = crate::db::sessions::parse_legacy_markdown_transcript(
                            first_msg.content.as_deref().unwrap(),
                        );
                        if !parsed.is_empty() {
                            history = parsed;
                        }
                    }
                }

                log::info!(
                    "Loaded {} messages from SQLite for session {}",
                    history.len(),
                    session_id
                );
            } else {
                let now = chrono::Utc::now().to_rfc3339();
                let session = crate::db::sessions::SessionRow {
                    id: session_id.clone(),
                    title: "Active Session".to_string(),
                    summary: None,
                    created_at: now.clone(),
                    updated_at: now.clone(),
                    active_skills: Some("[]".to_string()),
                };
                let _ = crate::db::sessions::insert_session(&store, &session);
            }
        }

        Self {
            history: Mutex::new(history),
            http_client,
            uploaded_files: Mutex::new(Vec::new()),
            backup_history: Mutex::new(None),
            session_id: Mutex::new(session_id),
            last_archived_hash: Mutex::new(last_archived_hash),
            app_handle,
        }
    }

    /// Called when the user deletes the currently-active session.
    /// Rotates to a new session ID without archiving (delete is intentional).
    pub async fn reset_for_delete(&self) -> String {
        let new_id = uuid::Uuid::new_v4().to_string();
        *self.session_id.lock().await = new_id.clone();
        *self.last_archived_hash.lock().await = 0;
        self.history.lock().await.clear();
        *self.backup_history.lock().await = None;
        new_id
    }

    pub async fn rewind_history(&self) {
        let mut history = self.history.lock().await;
        if history.is_empty() {
            return;
        }

        while let Some(msg) = history.pop() {
            if msg.role == "user" {
                break;
            }
        }

        // Release lock before persist
        drop(history);
        self.persist_history().await;
    }

    pub async fn save_and_clear_history(&self, api_key: Option<String>) {
        // Phase 1: Extract session data and decide whether to archive.
        // Do all of this in a scoped block so we don't hold these guards
        // while acquiring backup_history or uploaded_files below.
        let (history_clone, current_session_id, should_archive) = {
            let history = self.history.lock().await;
            let session_id_guard = self.session_id.lock().await;
            let last_hash_guard = self.last_archived_hash.lock().await;

            let history_clone = history.clone();
            let current_hash = self.calculate_history_hash(&history_clone);
            let should_archive = current_hash != *last_hash_guard;
            let current_session_id = session_id_guard.clone();

            (history_clone, current_session_id, should_archive)
        };

        // Phase 2: Archive on clear if changes occurred
        // Only run the archive if the hash changed since the last auto-archive.
        if should_archive {
            let app_handle_clone = self.app_handle.clone();
            let http_client_clone = self.http_client.clone();
            let session_id_for_task = current_session_id.clone();
            let history_for_task = history_clone.clone();

            tauri::async_runtime::spawn(async move {
                if let Ok(config) = crate::config::load_config(&app_handle_clone) {
                    if let Err(e) = crate::sessions::archive_session_transcript(
                        &app_handle_clone,
                        &http_client_clone,
                        &config,
                        &session_id_for_task,
                        history_for_task,
                    )
                    .await
                    {
                        log::warn!("[Agent] Failed to archive session on clear: {}", e);
                    }
                }
            });
        }

        // Phase 3: Update the hash, rotate session ID, clear history and backup.
        // Each accessed under its own scope to prevent simultaneous holding.
        {
            let mut last_hash_guard = self.last_archived_hash.lock().await;
            *last_hash_guard = if should_archive {
                self.calculate_history_hash(&history_clone)
            } else {
                0
            };
        }

        let new_session_id = uuid::Uuid::new_v4().to_string();
        {
            let mut session_id_guard = self.session_id.lock().await;
            *session_id_guard = new_session_id.clone();
        }

        // Initialize the new session in the DB immediately so FK constraints pass
        if let Ok(store) = crate::memories::get_vector_store(&self.app_handle) {
            let now = chrono::Utc::now().to_rfc3339();
            let session = crate::db::sessions::SessionRow {
                id: new_session_id.clone(),
                title: "Active Session".to_string(),
                summary: None,
                created_at: now.clone(),
                updated_at: now.clone(),
                active_skills: Some("[]".to_string()),
            };
            let _ = crate::db::sessions::insert_session(&store, &session);
        }

        {
            let mut backup = self.backup_history.lock().await;
            *backup = Some((history_clone, current_session_id));
        }

        {
            let mut history = self.history.lock().await;
            history.clear();
        }

        // Phase 4: Delete uploaded Gemini files in the background.
        let uris_to_delete: Vec<String> = {
            let mut uploaded_files = self.uploaded_files.lock().await;
            let uris = uploaded_files.clone();
            uploaded_files.clear();
            uris
        };

        if !uris_to_delete.is_empty() {
            if let Some(key) = api_key {
                for uri in uris_to_delete.iter() {
                    if let Some(file_name) = uri.split('/').last() {
                        let delete_url = format!(
                            "https://generativelanguage.googleapis.com/v1beta/files/{}",
                            file_name
                        );
                        let _ = self
                            .http_client
                            .delete(&delete_url)
                            .header("X-Goog-Api-Key", key.as_str())
                            .send()
                            .await;
                    }
                }
            }
        }

        // Phase 5: Persist the final cleared state to SQLite.
        self.persist_history().await;
    }

    pub async fn restore_history(&self) -> Result<(), String> {
        // Extract the saved state, then immediately release both guards to avoid
        // holding `history` + `backup` while acquiring `session_id` + `last_archived_hash`.
        let saved = {
            let mut backup = self.backup_history.lock().await;
            backup.take()
        };

        if let Some((saved_history, saved_session_id)) = saved {
            let new_hash = self.calculate_history_hash(&saved_history);

            {
                let mut history = self.history.lock().await;
                *history = saved_history;
            }
            {
                let mut session_id_guard = self.session_id.lock().await;
                *session_id_guard = saved_session_id;
            }
            {
                let mut last_hash_guard = self.last_archived_hash.lock().await;
                *last_hash_guard = new_hash;
            }

            self.persist_history().await;
            Ok(())
        } else {
            Err("No backup available".to_string())
        }
    }

    pub async fn get_history(&self) -> Vec<ChatMessage> {
        let history = self.history.lock().await;
        history.clone()
    }

    pub async fn load_session_from_db<R: Runtime>(
        &self,
        app_handle: &AppHandle<R>,
        session_id: &str,
    ) -> Result<(), String> {
        let new_history = {
            if let Ok(store) = crate::memories::get_vector_store(app_handle) {
                let mut stmt = store
                    .conn
                    .prepare(
                        "SELECT content FROM messages WHERE session_id = ? ORDER BY created_at ASC",
                    )
                    .map_err(|e| e.to_string())?;
                let mut history = Vec::new();
                if let Ok(msg_iter) = stmt.query_map([session_id], |row| {
                    let content: String = row.get(0)?;
                    Ok(
                        serde_json::from_str::<ChatMessage>(&content).unwrap_or_else(|_| {
                            ChatMessage {
                                role: "system".to_string(),
                                content: Some("Failed to parse message".to_string()),
                                reasoning: None,
                                tool_calls: None,
                                tool_call_id: None,
                                is_cron: None,
                                images: None,
                            }
                        }),
                    )
                }) {
                    for msg_res in msg_iter {
                        if let Ok(msg) = msg_res {
                            history.push(msg);
                        }
                    }
                }

                // Dynamic fallback for legacy markdown migrations
                if history.len() == 1 {
                    let first_msg = &history[0];
                    if first_msg.role == "assistant"
                        && first_msg
                            .content
                            .as_deref()
                            .unwrap_or("")
                            .starts_with("# Session Transcript")
                    {
                        let parsed = crate::db::sessions::parse_legacy_markdown_transcript(
                            first_msg.content.as_deref().unwrap(),
                        );
                        if !parsed.is_empty() {
                            history = parsed;
                        }
                    }
                }

                history
            } else {
                return Err("Failed to open database".to_string());
            }
        };

        let hash = self.calculate_history_hash(&new_history);
        *self.history.lock().await = new_history;
        *self.session_id.lock().await = session_id.to_string();
        *self.last_archived_hash.lock().await = hash;
        *self.backup_history.lock().await = None;

        Ok(())
    }

    pub async fn get_message_count(&self) -> usize {
        let history = self.history.lock().await;
        history.len()
    }

    pub async fn has_backup(&self) -> bool {
        let backup = self.backup_history.lock().await;
        backup.is_some()
    }

    /// Helper to compute a simple hash of the chat history to detect changes
    fn calculate_history_hash(&self, history: &[ChatMessage]) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        for msg in history {
            msg.role.hash(&mut hasher);
            if let Some(content) = &msg.content {
                content.hash(&mut hasher);
            }
            // Hash tool calls to catch edits there too
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

    /// Retry the last response with a hint about KaTeX errors
    /// Called by frontend when KaTeX parsing fails
    pub async fn retry_with_katex_hint<R: Runtime>(
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

                // Re-process with the hint
                // Note: We need to trigger a new processing loop without a new user message
                // This is handled by calling process_message with an empty message that gets ignored
                // Actually, we'll just re-use the existing flow by calling the internal method
                self.run_retry_turn(app_handle, config).await?;
            }
        }

        Ok(())
    }

    /// Internal method to run a retry turn after hint injection
    async fn run_retry_turn<R: Runtime>(
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
            .unwrap_or("gemini-2.5-flash-lite".to_string());

        let is_gemini = crate::models::is_gemini_model(&selected_model);

        let _continue_turn = if is_gemini {
            let api_key = config.gemini_api_key.as_ref().ok_or("No Gemini API key")?;
            self.process_gemini_turn(
                app_handle,
                config,
                &mut history,
                stream_id,
                &selected_model,
                api_key,
                None,  // No RAG context for retry
                false, // Not research mode
            )
            .await?
        } else {
            self.process_openrouter_turn(app_handle, config, &mut history, stream_id, None, false)
                .await?
        };

        // Persist the new response
        drop(history);
        self.persist_history().await;

        Ok(())
    }

    pub async fn persist_history(&self) {
        let history = self.history.lock().await;
        let session_id = self.session_id.lock().await;

        if let Ok(store) = crate::memories::get_vector_store(&self.app_handle) {
            // Delete existing messages for session
            let _ = store.conn.execute(
                "DELETE FROM messages WHERE session_id = ?",
                rusqlite::params![*session_id],
            );

            // Insert current history
            for msg in history.iter() {
                let msg_row = crate::db::sessions::MessageRow {
                    id: uuid::Uuid::new_v4().to_string(),
                    session_id: session_id.clone(),
                    role: msg.role.clone(),
                    content: serde_json::to_string(msg).unwrap_or_else(|_| "{}".to_string()),
                    created_at: chrono::Utc::now().to_rfc3339(),
                };
                let _ = crate::db::sessions::insert_message(&store, &msg_row);
            }

            // Update session updated_at
            let _ = store.conn.execute(
                "UPDATE sessions SET updated_at = ? WHERE id = ?",
                rusqlite::params![chrono::Utc::now().to_rfc3339(), *session_id],
            );
        }
    }

    async fn insert_single_message_to_db<R: Runtime>(
        &self,
        app_handle: &AppHandle<R>,
        msg: &ChatMessage,
    ) {
        if let Ok(store) = crate::memories::get_vector_store(app_handle) {
            let session_id = self.session_id.lock().await;
            let msg_row = crate::db::sessions::MessageRow {
                id: uuid::Uuid::new_v4().to_string(),
                session_id: session_id.clone(),
                role: msg.role.clone(),
                content: serde_json::to_string(msg).unwrap_or_else(|_| "{}".to_string()),
                created_at: chrono::Utc::now().to_rfc3339(),
            };
            let _ = crate::db::sessions::insert_message(&store, &msg_row);
            let _ = store.conn.execute(
                "UPDATE sessions SET updated_at = ? WHERE id = ?",
                rusqlite::params![chrono::Utc::now().to_rfc3339(), *session_id],
            );
        }
    }

    pub async fn process_message<R: Runtime>(
        &self,
        app_handle: &AppHandle<R>,
        message: String,
        images_base64: Option<Vec<String>>,
        images_mime_types: Option<Vec<String>>,
        config: &crate::config::AppConfig,
        is_cron: bool,
    ) -> Result<(), String> {
        println!("process_message called. Message len: {}", message.len());

        let mut history = self.history.lock().await;

        // Determine model type using the centralized registry
        let selected_model = config
            .selected_model
            .clone()
            .unwrap_or("gemini-2.5-flash-lite".to_string());
        let is_gemini = crate::models::is_gemini_model(&selected_model);
        let has_native_vision = crate::models::model_supports_vision(&selected_model);

        // Process images with 3-way routing:
        //   1. Gemini: Upload via Files API (native multimodal with file URIs)
        //   2. Vision-capable OpenRouter: Store base64, pass natively via to_multimodal_messages()
        //   3. Non-vision model: Vision LLM fallback for text description + store base64 for history
        let mut image_descriptions: Vec<String> = Vec::new();
        let uploaded_images: Option<Vec<ImageAttachment>> = if let (Some(bases), Some(mimes)) =
            (images_base64.as_ref(), images_mime_types.as_ref())
        {
            if bases.is_empty() {
                None
            } else {
                let mut attachments = Vec::with_capacity(bases.len());

                for (img_data, mime_type) in bases.iter().zip(mimes.iter()) {
                    let file_uri = if is_gemini {
                        // Gemini: Upload to Files API for native multimodal
                        match crate::gemini_files::upload_image_to_gemini_files_api(
                            &self.http_client,
                            img_data,
                            mime_type,
                            config.gemini_api_key.as_ref().ok_or("No Gemini API key")?,
                        )
                        .await
                        {
                            Ok(file_uri) => {
                                self.uploaded_files
                                    .lock()
                                    .await
                                    .push(file_uri.file_uri.clone());
                                Some(file_uri.file_uri)
                            }
                            Err(e) => {
                                return Err(format!(
                                    "Failed to upload image to Gemini Files API: {}",
                                    e
                                ))
                            }
                        }
                    } else if has_native_vision {
                        // Vision-capable OpenRouter model: images will be sent natively
                        // via to_multimodal_messages() as inline data URIs
                        log::info!(
                            "[Agent] Model {} supports vision — sending image natively",
                            selected_model
                        );
                        None
                    } else {
                        // Non-vision model: use Vision LLM to produce text description
                        match crate::integrations::vision_llm::process_image_with_context(
                            &self.http_client,
                            img_data,
                            mime_type,
                            &message,
                            config,
                        )
                        .await
                        {
                            Ok(contextual_response) => {
                                log::info!(
                                    "[Agent] Vision LLM contextual response: {} chars",
                                    contextual_response.len()
                                );
                                image_descriptions.push(contextual_response);
                            }
                            Err(e) => {
                                log::warn!(
                                    "[Agent] Vision LLM contextual processing failed: {}",
                                    e
                                );
                                image_descriptions
                                    .push("[Image attached but could not be analyzed]".to_string());
                            }
                        }
                        None
                    };

                    // Always store image data on the attachment for history fidelity
                    attachments.push(ImageAttachment {
                        base64: img_data.clone(),
                        mime_type: mime_type.clone(),
                        file_uri,
                    });
                }

                Some(attachments)
            }
        } else {
            None
        };

        // For non-vision models, prepend contextual image analysis to the message
        let augmented_message = if !is_gemini && !has_native_vision && !image_descriptions.is_empty()
        {
            let analysis = image_descriptions.join("\n\n");
            format!(
                "[Visual Analysis]\n{}\n\n[User Message]\n{}",
                analysis, message
            )
        } else {
            message.clone()
        };

        let msg = ChatMessage {
            role: "user".to_string(),
            content: Some(augmented_message),
            reasoning: None,
            tool_calls: None,
            tool_call_id: None,
            images: uploaded_images,
            is_cron: if is_cron { Some(true) } else { None },
        };
        history.push(msg.clone());
        self.insert_single_message_to_db(app_handle, &msg).await;

        if !is_cron {
            let json_msg = serde_json::to_string(&msg).unwrap_or_default();
            app_handle.emit("user-message", json_msg).ok();
        }

        // Incognito mode: skip all RAG/memory retrieval and storage
        let incognito = config.incognito_mode.unwrap_or(false);

        // RAG: Generate embedding and retrieve relevant interactions using hybrid search (BM25 + Dense + RRF)
        // Skip in incognito mode to avoid using previous context
        let user_embedding = if !incognito {
            if let Some(api_key) = &config.gemini_api_key {
                crate::interactions::generate_embedding(&self.http_client, &message, api_key)
                    .await
                    .ok()
            } else {
                None
            }
        } else {
            None
        };

        let relevant_interactions = if let Some(emb) = &user_embedding {
            // Use hybrid search with RRF fusion of BM25 and dense results
            crate::interactions::hybrid_search_interactions(
                app_handle, &message, emb, /* limit= */ 5,
            )
            .unwrap_or_default()
        } else {
            Vec::new()
        };

        let mut rag_context_str = if !relevant_interactions.is_empty() {
            let mut s = String::from("\n\nRelevant Past Interactions:\n");
            for entry in relevant_interactions {
                s.push_str(&format!(
                    "- [{}] {}: {}\n",
                    entry.ts.format("%Y-%m-%d"),
                    entry.role,
                    entry.content
                ));
            }
            Some(s)
        } else {
            None
        };

        // RAG: Context from Topics or Insights (Hybrid Vector/FTS)
        if let Some(emb) = &user_embedding {
            let handle = app_handle.clone();
            let msg = message.clone();
            let embedding = emb.clone();
            let context_res = match tokio::task::spawn_blocking(move || {
                crate::memories::find_relevant_context(&handle, &msg, &embedding)
            })
            .await
            {
                Ok(res) => res,
                Err(e) => {
                    log::error!("[Agent] Context lookup task panicked: {}", e);
                    Ok(None) // Gracefully degrade — continue without context
                }
            };

            if let Ok(Some((name, content, is_insight))) = context_res {
                let s = rag_context_str.get_or_insert_with(String::new);
                if is_insight {
                    s.push_str("\n\nRelevant Insight:\n");
                    s.push_str(&format!("### Insight: {}\n{}\n\n", name, content));
                    log::debug!("[Agent] Using insight: {}", name);
                } else {
                    s.push_str("\n\nRelevant Topic Summary:\n");
                    s.push_str(&format!("### Topic: {}\n{}\n\n", name, content));
                    log::debug!("[Agent] Using topic: {}", name);
                }
            }
        }

        // ====================================================================
        // Compaction: Check if we're approaching context window limits
        // ====================================================================
        let compaction_enabled = config.enable_compaction.unwrap_or(true) && !incognito;
        log::info!(
            "[Agent] Compaction check: enabled={}, incognito={}, history_len={}",
            compaction_enabled,
            incognito,
            history.len()
        );

        if compaction_enabled {
            let selected_model = config
                .selected_model
                .clone()
                .unwrap_or("gemini-2.5-flash-lite".to_string());
            let threshold = config.compaction_threshold;

            let current_tokens = crate::compaction::estimate_history_tokens(&history);
            let context_size = crate::compaction::get_context_size(&selected_model);
            let threshold_pct = threshold.unwrap_or(crate::compaction::DEFAULT_THRESHOLD);
            let threshold_tokens = (context_size as f32 * threshold_pct) as usize;

            log::info!(
                "[Agent] Compaction: model={}, tokens={}, context={}, threshold={}% ({} tokens)",
                selected_model,
                current_tokens,
                context_size,
                (threshold_pct * 100.0) as u32,
                threshold_tokens
            );

            if crate::compaction::should_compact(&history, &selected_model, Some(threshold_pct)) {
                log::info!(
                    "[Agent] Context approaching {}% of window - triggering compaction",
                    (threshold_pct * 100.0) as u32
                );

                // Emit compaction event for UI feedback
                let compaction_event = serde_json::json!({
                    "status": "starting",
                    "history_len": history.len()
                });
                app_handle
                    .emit("agent-compaction", compaction_event.to_string())
                    .ok();

                // Pre-compaction flush: extract important facts before summarization
                match crate::compaction::pre_compaction_flush(
                    app_handle,
                    &self.http_client,
                    config,
                    &history,
                )
                .await
                {
                    Ok(flush_result) => {
                        if flush_result.extracted {
                            log::info!(
                                "[Agent] Pre-compaction flush: {} facts saved to daily log",
                                flush_result.fact_count
                            );
                        }
                    }
                    Err(e) => {
                        log::warn!("[Agent] Pre-compaction flush failed: {}", e);
                        // Continue with compaction even if flush fails
                    }
                }

                // Compact history
                match crate::compaction::compact_history(
                    app_handle,
                    &self.http_client,
                    config,
                    &mut history,
                )
                .await
                {
                    Ok(result) => {
                        log::info!(
                            "[Agent] Compacted {} turns, preserved {}, saved ~{} tokens",
                            result.compacted_turns,
                            result.preserved_turns,
                            result.tokens_saved
                        );

                        // Emit completion event
                        let complete_event = serde_json::json!({
                            "status": "complete",
                            "compacted_turns": result.compacted_turns,
                            "preserved_turns": result.preserved_turns,
                            "tokens_saved": result.tokens_saved
                        });
                        app_handle
                            .emit("agent-compaction", complete_event.to_string())
                            .ok();
                    }
                    Err(e) => {
                        log::error!("[Agent] Compaction failed: {}", e);
                        // Continue processing without compaction
                    }
                }
            } else {
                log::info!(
                    "[Agent] Compaction not needed: {} < {} tokens",
                    current_tokens,
                    threshold_tokens
                );
            }
        }

        app_handle.emit("agent-processing-start", ()).ok();
        let stream_id =
            crate::CURRENT_STREAM_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;

        // Detect research mode: either from config OR dynamically via intent classification
        let is_research_mode = if config.research_mode.unwrap_or(false) {
            true
        } else if let Some(api_key) = config.gemini_api_key.as_ref() {
            // Dynamically detect research queries using LLM
            if let Some(last_msg) = history.last() {
                if last_msg.role == "user" {
                    self.classify_intent(&last_msg.content.clone().unwrap_or_default(), api_key)
                        .await
                        .unwrap_or(false)
                } else {
                    false
                }
            } else {
                false
            }
        } else {
            false
        };

        if is_research_mode {
            log::info!("[Agent] Research mode detected - using extended turn limit");
        }

        let max_turns = if is_research_mode { 15 } else { 5 };
        let mut current_turn = 0;

        // Auto-retry state
        let max_retries = config.max_auto_retries.unwrap_or(2);
        let retry_on_empty = config.retry_on_empty.unwrap_or(true);
        let mut retry_count = 0u32;
        let mut pending_retry_hint: Option<String> = None;

        loop {
            if current_turn >= max_turns {
                break;
            }
            current_turn += 1;

            let selected_model = config
                .selected_model
                .clone()
                .unwrap_or("gemini-2.5-flash-lite".to_string());

            // Detect provider using centralized model registry
            let is_gemini = crate::models::is_gemini_model(&selected_model);

            // Inject retry hint if pending (from previous failed attempt)
            if let Some(hint) = pending_retry_hint.take() {
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
            }

            let continue_turn = if is_gemini {
                let api_key = config.gemini_api_key.as_ref().ok_or("No Gemini API key")?;
                self.process_gemini_turn(
                    app_handle,
                    config,
                    &mut history,
                    stream_id,
                    &selected_model,
                    api_key,
                    rag_context_str.as_deref(),
                    is_research_mode,
                )
                .await?
            } else {
                // Both OpenRouter and Cerebras use OpenAI-compatible API
                self.process_openrouter_turn(
                    app_handle,
                    config,
                    &mut history,
                    stream_id,
                    rag_context_str.as_deref(),
                    is_research_mode,
                )
                .await?
            };

            // Check if we need to retry (empty response with reasoning)
            if !continue_turn && retry_on_empty && retry_count < max_retries {
                if let Some(last_msg) = history.last() {
                    let has_reasoning = last_msg
                        .reasoning
                        .as_ref()
                        .map(|r| !r.is_empty())
                        .unwrap_or(false);
                    let has_content = last_msg
                        .content
                        .as_ref()
                        .map(|c| !c.trim().is_empty())
                        .unwrap_or(false);
                    let has_tools = last_msg.tool_calls.is_some();

                    // Retry if: has reasoning but no content and no tool calls
                    if has_reasoning && !has_content && !has_tools {
                        retry_count += 1;
                        log::info!(
                            "[Agent] Empty response with reasoning detected, retry {}/{}",
                            retry_count,
                            max_retries
                        );

                        // Emit retry event to frontend
                        let retry_event = serde_json::json!({
                            "reason": "empty_response",
                            "attempt": retry_count,
                            "max": max_retries
                        });
                        app_handle.emit("agent-retry", retry_event.to_string()).ok();

                        // Pop the failed response from history
                        history.pop();

                        // Set up retry hint for next iteration
                        pending_retry_hint = Some(RetryReason::EmptyResponse.get_hint());

                        // Don't break - continue the loop for retry
                        continue;
                    }
                }
            }

            // Notify frontend when retries are exhausted
            if !continue_turn && retry_count >= max_retries && retry_count > 0 {
                let exhausted_event = serde_json::json!({
                    "reason": "empty_response",
                    "attempts": retry_count,
                    "max": max_retries
                });
                app_handle
                    .emit("agent-retry-exhausted", exhausted_event.to_string())
                    .ok();
            }

            if !continue_turn {
                break;
            }
        }

        // Log interactions for future RAG (skip in incognito mode - use variable defined earlier)
        if !incognito {
            // 1. Log user message
            if let Some(emb) = user_embedding {
                crate::interactions::log_interaction(app_handle, "user", &message, Some(emb))
                    .await
                    .ok();
            }

            // 2. Log assistant response
            if let Some(last_msg) = history.last() {
                if (last_msg.role == "model" || last_msg.role == "assistant")
                    && last_msg.content.is_some()
                {
                    let content = last_msg.content.as_ref().unwrap();
                    let response_embedding = if let Some(api_key) = &config.gemini_api_key {
                        crate::interactions::generate_embedding(&self.http_client, content, api_key)
                            .await
                            .ok()
                    } else {
                        None
                    };
                    crate::interactions::log_interaction(
                        app_handle,
                        "model",
                        content,
                        response_embedding,
                    )
                    .await
                    .ok();
                }
            }
        }

        // Persist history to disk after each message exchange (always, regardless of incognito RAG)
        drop(history); // Release lock before persist
        self.persist_history().await;

        // ── Auto-archive: generate session title + summary after 2 user + 2 agent messages ──
        // Fires once per content change when the session crosses the 2+2 threshold.
        // Uses last_archived_hash so it won't re-fire on every subsequent message if
        // the content matches what was previously archived (e.g. no new messages since last archive).
        if !incognito {
            let history_snapshot = self.history.lock().await.clone();
            let user_msgs = history_snapshot.iter().filter(|m| m.role == "user").count();
            let asst_msgs = history_snapshot
                .iter()
                .filter(|m| m.role == "assistant" || m.role == "model")
                .count();

            if user_msgs >= 2 && asst_msgs >= 2 {
                let current_hash = self.calculate_history_hash(&history_snapshot);
                let last_hash = *self.last_archived_hash.lock().await;

                if current_hash != last_hash {
                    // Update the hash eagerly to prevent concurrent duplicate archives
                    *self.last_archived_hash.lock().await = current_hash;

                    let session_id_now = self.session_id.lock().await.clone();
                    let app_handle_clone = self.app_handle.clone();
                    let http_client_clone = self.http_client.clone();
                    tauri::async_runtime::spawn(async move {
                        if let Ok(config) = crate::config::load_config(&app_handle_clone) {
                            if let Err(e) = crate::sessions::archive_session_transcript(
                                &app_handle_clone,
                                &http_client_clone,
                                &config,
                                &session_id_now,
                                history_snapshot,
                            )
                            .await
                            {
                                log::warn!("[Agent] Auto-archive failed: {}", e);
                            } else {
                                log::info!("[Agent] Auto-archived session after 2+2 messages");
                            }
                        }
                    });
                }
            }
        }

        Ok(())
    }

    async fn execute_tool<R: Runtime>(
        &self,
        app_handle: &AppHandle<R>,
        function_name: &str,
        args: &Value,
        config: &crate::config::AppConfig,
    ) -> String {
        // Check cache first for cacheable tools
        if let Some(cached) = crate::cache::get_cached_result(app_handle, function_name, args) {
            log::info!(
                "[Tool] Cache HIT for {} - returning cached result",
                function_name
            );
            return cached;
        }

        let result = self
            .execute_tool_uncached(app_handle, function_name, args, config)
            .await;

        // Cache the result if eligible (never cache errors)
        if !result.starts_with("Error") {
            crate::cache::cache_result(app_handle, function_name, args, &result);
        }

        result
    }

    /// The actual tool execution logic (separated for caching wrapper)
    async fn execute_tool_uncached<R: Runtime>(
        &self,
        app_handle: &AppHandle<R>,
        function_name: &str,
        args: &Value,
        config: &crate::config::AppConfig,
    ) -> String {
        match function_name {
            "get_weather" => {
                let location = args["location"].as_str().unwrap_or_default();
                match perform_weather_lookup(&self.http_client, location).await {
                    Ok(Some((temp, unit, loc))) => format!("Weather in {}: {} {}", loc, temp, unit),
                    Ok(None) => "Weather data not found.".to_string(),
                    Err(e) => format!("Error: {}", e),
                }
            }
            "search_wikipedia" => {
                let query = args["query"].as_str().unwrap_or_default();
                match perform_wikipedia_lookup(&self.http_client, query).await {
                    Ok(Some((title, summary, _))) => {
                        format!("Wikipedia Title: {}\nSummary: {}", title, summary)
                    }
                    Ok(None) => "No Wikipedia results found.".to_string(),
                    Err(e) => format!("Error: {}", e),
                }
            }
            "get_stock_price" => {
                let symbol = args["symbol"].as_str().unwrap_or_default();
                perform_finance_lookup(symbol)
                    .await
                    .unwrap_or_else(|e| format!("Error: {}", e))
            }
            "search_arxiv" => {
                let query = args["query"].as_str().unwrap_or_default();
                match perform_arxiv_lookup(&self.http_client, query, 3).await {
                    Ok(papers) => {
                        let summaries: Vec<String> = papers
                            .iter()
                            .map(|p| {
                                format!(
                                    "- [{}] {} ({}): {}",
                                    p.id,
                                    p.title,
                                    p.published_date.as_deref().unwrap_or("?"),
                                    p.summary
                                )
                            })
                            .collect();
                        format!("ArXiv Results:\n{}", summaries.join("\n\n"))
                    }
                    Err(e) => format!("Error: {}", e),
                }
            }
            "read_arxiv_paper" => {
                let paper_id = args["paper_id"].as_str().unwrap_or_default();
                match read_arxiv_paper(&self.http_client, paper_id).await {
                    Ok(paper) => {
                        format!(
                            "# {}\n\n**Abstract:** {}\n\n{}",
                            paper.title, paper.abstract_text, paper.content
                        )
                    }
                    Err(e) => format!("Error reading paper: {}", e),
                }
            }
            "web_search" => {
                let query = args["query"].as_str().unwrap_or_default();
                match perform_web_search(query, config.brave_api_key.as_deref()).await {
                    Ok(results) => {
                        // Full format with snippets for the model to understand
                        let snippets: Vec<String> = results
                            .iter()
                            .map(|r| format!("- [{}]({}) : {}", r.title, r.url, r.snippet))
                            .collect();
                        format!("Web Search Results:\n{}", snippets.join("\n\n"))
                    }
                    Err(e) => format!("Error: {}", e),
                }
            }
            "open_url" => {
                let url = args["url"].as_str().unwrap_or_default();
                match crate::integrations::browser::read_url(&self.http_client, url).await {
                    Ok(markdown) => {
                        format!("Read URL Results for {}:\n\n{}", url, markdown)
                    }
                    Err(e) => format!("Error reading URL: {}", e),
                }
            }
            "youtube_transcript" => {
                let video = args["video"].as_str().unwrap_or_default();
                let video_id = match crate::integrations::youtube::extract_video_id(video) {
                    Some(id) => id,
                    None => return format!("Error: Could not extract a YouTube video ID from '{}'", video),
                };
                match crate::integrations::youtube::fetch_transcript(&self.http_client, &video_id).await {
                    Ok(result) => {
                        let formatted = crate::integrations::youtube::format_transcript(
                            &result.segments,
                            result.title.as_deref(),
                            result.channel.as_deref(),
                        );
                        let title_label = result.title.as_deref().unwrap_or(&video_id);
                        let video_link = format!("https://youtu.be/{}", video_id);
                        // Truncate very long transcripts to avoid blowing up context,
                        // but generate a chunked LLM summary of the full content so nothing is lost.
                        let char_count = formatted.chars().count();
                        if char_count > 30_000 {
                            // Find byte offset of the 30,000th character
                            let truncate_at = formatted
                                .char_indices()
                                .nth(30_000)
                                .map(|(i, _)| i)
                                .unwrap_or(formatted.len());

                            // Summarize the full transcript via background LLM (chunked for long transcripts)
                            let summary = self.summarize_long_transcript(config, &formatted, title_label).await;

                            let summary_section = match &summary {
                                Ok(s) => format!(
                                    "\n\n--- LLM Summary of Full Video ---\n\n{}\n\n--- End Summary ---",
                                    s
                                ),
                                Err(e) => {
                                    log::warn!("[YouTube] Failed to summarize transcript: {}", e);
                                    String::new()
                                }
                            };

                            format!(
                                "YouTube Transcript — {} ({})\n{} segments, truncated\n\n{}...\n\n[Transcript truncated at ~30,000 chars. Total length: {} chars]{}",
                                title_label,
                                video_link,
                                result.segments.len(),
                                &formatted[..truncate_at],
                                char_count,
                                summary_section,
                            )
                        } else {
                            format!(
                                "YouTube Transcript — {} ({})\n{} segments\n\n{}",
                                title_label,
                                video_link,
                                result.segments.len(),
                                formatted
                            )
                        }
                    }
                    Err(e) => format!("Error fetching transcript: {}", e),
                }
            }
            "save_memory" => {
                // Block in incognito mode
                if config.incognito_mode.unwrap_or(false) {
                    return "Skipped: Memory saving is disabled in incognito mode.".to_string();
                }
                // Quiet tool - no UI feedback, just log
                let category_str = args["category"].as_str().unwrap_or("fact");
                let content = args["content"].as_str().unwrap_or_default().to_string();
                let importance = args["importance"].as_u64().unwrap_or(3) as u8;

                let category = match category_str {
                    "preference" => crate::memories::MemoryCategory::Preference,
                    "project" => crate::memories::MemoryCategory::Project,
                    "interaction" => crate::memories::MemoryCategory::Interaction,
                    _ => crate::memories::MemoryCategory::Fact,
                };

                match crate::memories::add_memory(app_handle, category, content.clone(), importance)
                {
                    Ok(_) => format!("Memory saved: {}", content),
                    Err(e) => format!("Failed to save memory: {}", e),
                }
            }
            "update_topic_summary" => {
                // Block in incognito mode
                if config.incognito_mode.unwrap_or(false) {
                    return "Skipped: Topic updates are disabled in incognito mode.".to_string();
                }
                let topic = args["topic"].as_str().unwrap_or_default();
                let content = args["content"].as_str().unwrap_or_default();
                match crate::memories::update_topic_summary(app_handle, topic, content) {
                    Ok(_) => format!(
                        "Topic summary updated: {}. Note: Run `refresh_memories` to rebuild the search index for this change to appear in retrieval.",
                        topic
                    ),
                    Err(e) => format!("Failed to update topic summary: {}", e),
                }
            }
            "read_topic_summary" => {
                // Allow reading in incognito mode (no persistence)
                let topic = args["topic"].as_str().unwrap_or_default();
                match crate::memories::read_topic_summary(app_handle, topic) {
                    Ok(content) => content,
                    Err(e) => format!("Failed to read topic summary: {}", e),
                }
            }
            "refresh_memories" => {
                // Block in incognito mode
                if config.incognito_mode.unwrap_or(false) {
                    return "Skipped: Memory refresh is disabled in incognito mode.".to_string();
                }
                match crate::background::run_summary_job_from_agent(app_handle).await {
                    Ok(result) => {
                        let mut msg = format!(
                            "Memory refresh complete: {} topics updated, {} insights created",
                            result.topics_updated.len(),
                            result.insights_created.len()
                        );
                        if !result.topics_updated.is_empty() {
                            msg.push_str(&format!(
                                "\nTopics: {}",
                                result.topics_updated.join(", ")
                            ));
                        }
                        if !result.insights_created.is_empty() {
                            msg.push_str(&format!(
                                "\nInsights: {}",
                                result.insights_created.join(", ")
                            ));
                        }
                        msg
                    }
                    Err(e) => format!("Memory refresh failed: {}", e),
                }
            }
            "memory_search" => {
                let query = args["query"].as_str().unwrap_or_default();
                let max_results = args["max_results"].as_u64().unwrap_or(5) as usize;
                let min_score = args["min_score"].as_f64().unwrap_or(0.3) as f32;
                let time_filter = args["time_filter"].as_str();

                if let Some(tf) = time_filter {
                    if !tf.is_empty() {
                        let handle = app_handle.clone();
                        let query_str = query.to_string();
                        let tf_str = tf.to_string();
                        return tokio::task::spawn_blocking(move || {
                            if let Ok(store) = crate::memories::get_vector_store(&handle) {
                                crate::db::sessions::search_sessions_by_time(
                                    &store,
                                    &query_str,
                                    &tf_str,
                                    max_results,
                                )
                                .unwrap_or_else(|e| format!("Error searching sessions: {}", e))
                            } else {
                                "Error: Failed to open database".to_string()
                            }
                        })
                        .await
                        .unwrap_or_else(|e| e.to_string());
                    }
                }

                if query.is_empty() {
                    return "Error: query parameter is required".to_string();
                }

                // Generate embedding for the query
                let api_key = match config.gemini_api_key.as_ref() {
                    Some(key) => key.clone(),
                    None => return "Error: memory_search requires a Gemini API key for embedding generation".to_string(),
                };

                let embedding = match crate::interactions::generate_embedding(
                    &self.http_client,
                    query,
                    &api_key,
                )
                .await
                {
                    Ok(emb) => emb,
                    Err(e) => return format!("Error generating query embedding: {}", e),
                };

                // Run hybrid search on a blocking thread (SQLite is sync)
                let handle = app_handle.clone();
                let query_text = query.to_string();
                let search_result = match tokio::task::spawn_blocking(move || {
                    crate::memories::search_memory_chunks(
                        &handle,
                        &query_text,
                        &embedding,
                        max_results,
                        min_score,
                    )
                })
                .await
                {
                    Ok(res) => res,
                    Err(e) => return format!("Error: search task panicked: {}", e),
                };

                match search_result {
                    Ok(chunks) => {
                        if chunks.is_empty() {
                            return "No matching memories found.".to_string();
                        }
                        let results: Vec<serde_json::Value> = chunks
                            .iter()
                            .map(|c| {
                                let source_dir = match c.source_type {
                                    crate::memories::SourceType::Topic => "topics",
                                    crate::memories::SourceType::Insight => "insights",
                                    crate::memories::SourceType::Session => "sessions",
                                };
                                serde_json::json!({
                                    "source": format!("{:?}", c.source_type).to_lowercase(),
                                    "path": format!("{}/{}.md", source_dir, c.source_name),
                                    "heading": c.heading,
                                    "start_line": c.start_line,
                                    "end_line": c.end_line,
                                    "snippet": c.text.chars().take(500).collect::<String>(),
                                })
                            })
                            .collect();
                        serde_json::to_string_pretty(&results)
                            .unwrap_or_else(|_| "Error formatting results".to_string())
                    }
                    Err(e) => format!("Memory search failed: {}", e),
                }
            }
            "memory_get" => {
                let session_id = args["session_id"].as_str();
                if let Some(sid) = session_id {
                    let handle = app_handle.clone();
                    let sid_str = sid.to_string();
                    return tokio::task::spawn_blocking(move || {
                        if let Ok(store) = crate::memories::get_vector_store(&handle) {
                            crate::db::sessions::get_session_transcript(&store, &sid_str)
                                .map(|t| format!("Session {} transcript:\n{}", sid_str, t))
                                .unwrap_or_else(|e| format!("Error getting transcript: {}", e))
                        } else {
                            "Error: Failed to open database".to_string()
                        }
                    })
                    .await
                    .unwrap_or_else(|e| e.to_string());
                }

                let path = args["path"].as_str().unwrap_or_default();
                let from = args["from"].as_u64().unwrap_or(1) as usize;
                let lines = args["lines"].as_u64().unwrap_or(50).min(200) as usize;

                if path.is_empty() {
                    return "Error: path parameter is required".to_string();
                }

                match crate::memories::read_memory_file_lines(app_handle, path, from, lines) {
                    Ok(content) => content,
                    Err(e) => format!("Error: {}", e),
                }
            }
            "list_skills" => {
                let skills = crate::skills::list_available_skills();
                if skills.is_empty() {
                    "No dynamic skills are currently available in the workspace.".to_string()
                } else {
                    format!("Available skills:\n{}", skills.join("\n"))
                }
            }
            "load_skill" => {
                let name = args["name"].as_str().unwrap_or_default();
                if let Some(_content) = crate::skills::get_skill_content(name) {
                    let session_id = self.session_id.lock().await.clone();
                    if let Ok(store) = crate::memories::get_vector_store(app_handle) {
                        if let Ok(mut active_skills) = crate::db::sessions::get_active_skills(&store, &session_id) {
                            if !active_skills.contains(&name.to_string()) {
                                active_skills.push(name.to_string());
                                let skills_json = serde_json::to_string(&active_skills).unwrap_or_else(|_| "[]".to_string());
                                let _ = crate::db::sessions::update_active_skills(&store, &session_id, &skills_json);
                                format!("Successfully loaded skill '{}'. The instructions will be active for the rest of this session.", name)
                            } else {
                                format!("Skill '{}' is already active.", name)
                            }
                        } else {
                            "Failed to retrieve active session skills.".to_string()
                        }
                    } else {
                        "Failed to access database.".to_string()
                    }
                } else {
                    format!("Skill '{}' not found. Use `list_skills` to see what is available.", name)
                }
            }
            "unload_skill" => {
                let name = args["name"].as_str().unwrap_or_default();
                let session_id = self.session_id.lock().await.clone();
                if let Ok(store) = crate::memories::get_vector_store(app_handle) {
                    if let Ok(mut active_skills) = crate::db::sessions::get_active_skills(&store, &session_id) {
                        if active_skills.contains(&name.to_string()) {
                            active_skills.retain(|s| s != name);
                            let skills_json = serde_json::to_string(&active_skills).unwrap_or_else(|_| "[]".to_string());
                            let _ = crate::db::sessions::update_active_skills(&store, &session_id, &skills_json);
                            format!("Successfully unloaded skill '{}'.", name)
                        } else {
                            format!("Skill '{}' is not currently active.", name)
                        }
                    } else {
                        "Failed to retrieve active session skills.".to_string()
                    }
                } else {
                    "Failed to access database.".to_string()
                }
            }
            "run_python" => {
                let code = args["code"].as_str().unwrap_or_default();
                if code.trim().is_empty() {
                    return "Error: No code provided.".to_string();
                }

                let resource_dir = app_handle
                    .path()
                    .resource_dir()
                    .unwrap_or_default();

                match crate::sandbox::execute_python(code, resource_dir, 30).await {
                    Ok(result) => {
                        let mut output = String::new();
                        if result.timed_out {
                            output.push_str("**Execution timed out (30s limit)**\n\n");
                        }
                        if result.fuel_exhausted {
                            output.push_str("**Execution halted: instruction limit reached**\n\n");
                        }
                        if !result.stdout.is_empty() {
                            output.push_str("**stdout:**\n```\n");
                            if result.stdout.len() > 20_000 {
                                output.push_str(&result.stdout[..20_000]);
                                output.push_str(&format!(
                                    "\n```\n[Truncated at 20,000 chars. Total: {} chars]\n",
                                    result.stdout.len()
                                ));
                            } else {
                                output.push_str(&result.stdout);
                                output.push_str("\n```\n");
                            }
                        }
                        if !result.stderr.is_empty() {
                            output.push_str("**stderr:**\n```\n");
                            if result.stderr.len() > 5_000 {
                                output.push_str(&result.stderr[..5_000]);
                                output.push_str("\n```\n[stderr truncated]\n");
                            } else {
                                output.push_str(&result.stderr);
                                output.push_str("\n```\n");
                            }
                        }
                        if output.is_empty() {
                            output.push_str("Code executed successfully with no output.");
                        }
                        output
                    }
                    Err(e) => format!("Error: {}", e),
                }
            }
            _ => format!("Unknown tool: {}", function_name),
        }
    }

    async fn classify_intent(&self, query: &str, api_key: &str) -> Result<bool, String> {
        let url = "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash-lite:generateContent";

        let payload = serde_json::json!({
            "contents": [{
                "parts": [{
                    "text": format!("{}\n\nQuery: {}", crate::prompts::INTENT_CLASSIFICATION_PROMPT, query)
                }]
            }],
            "generationConfig": {
                "temperature": 0.0,
                "maxOutputTokens": 10
            }
        });

        let client = reqwest::Client::new();
        let res = client
            .post(url)
            .header("X-Goog-Api-Key", api_key)
            .json(&payload)
            .send()
            .await
            .map_err(|e| e.to_string())?;

        if !res.status().is_success() {
            return Err(format!("Intent classification failed: {}", res.status()));
        }

        let body: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;

        if let Some(candidates) = body.get("candidates").and_then(|c| c.as_array()) {
            if let Some(first) = candidates.first() {
                if let Some(content) = first.get("content") {
                    if let Some(parts) = content.get("parts").and_then(|p| p.as_array()) {
                        if let Some(text_part) = parts.first() {
                            if let Some(text) = text_part.get("text").and_then(|t| t.as_str()) {
                                return Ok(text.trim().to_uppercase().contains("YES"));
                            }
                        }
                    }
                }
            }
        }

        Ok(false)
    }

    /// Summarize a long YouTube transcript using the background LLM.
    ///
    /// For transcripts that exceed the background model's context window, splits the
    /// transcript into chunks, summarizes each independently, then produces a final
    /// combined summary. This ensures no information is lost regardless of transcript length.
    ///
    /// Chunk size is set conservatively at 80,000 chars (~20K tokens) to leave room for
    /// the system prompt and response within the 128K-token context of background models.
    async fn summarize_long_transcript(
        &self,
        config: &crate::config::AppConfig,
        full_transcript: &str,
        title: &str,
    ) -> Result<String, String> {
        let model = config
            .background_model
            .as_deref()
            .unwrap_or("gpt-oss-120b (Groq)");

        log::info!(
            "[YouTube] Summarizing long transcript ({} chars) for \"{}\" via {}",
            full_transcript.chars().count(),
            title,
            model
        );

        // ~20K tokens worth of transcript per chunk, leaving headroom for system prompt + response
        const CHUNK_SIZE: usize = 80_000;

        // Split transcript into chunks at UTF-8-safe boundaries
        let chunks = self.split_transcript_chunks(full_transcript, CHUNK_SIZE);

        if chunks.len() == 1 {
            // Small enough to summarize in one shot
            let system_prompt = "You are a precise summarization assistant. Given a full YouTube video transcript, produce a comprehensive summary that captures ALL key points, arguments, examples, and conclusions. Organize the summary with clear sections. Do not omit any important topics or details — the user will only see the first portion of the timestamped transcript plus your summary for the rest.";
            let user_message = format!(
                "Summarize the following YouTube video transcript comprehensively. The video is titled \"{}\".\n\n---\n{}",
                title, full_transcript
            );
            return crate::background::call_llm_oneshot(
                &self.http_client, config, model, system_prompt, &user_message, 4000, 0.3,
            ).await;
        }

        // Multi-chunk: summarize each chunk, then combine
        log::info!(
            "[YouTube] Transcript split into {} chunks for summarization",
            chunks.len()
        );

        let chunk_system = "You are a precise summarization assistant. You will receive one section of a YouTube video transcript. Produce a detailed summary of THIS section only, capturing all key points, arguments, examples, data, and conclusions. Be thorough — your output will be combined with summaries of other sections.";

        let mut chunk_summaries = Vec::with_capacity(chunks.len());
        for (i, chunk) in chunks.iter().enumerate() {
            let user_message = format!(
                "Summarize section {} of {} from the YouTube video \"{}\". Capture every important detail.\n\n---\n{}",
                i + 1,
                chunks.len(),
                title,
                chunk
            );

            let summary = crate::background::call_llm_oneshot(
                &self.http_client, config, model, chunk_system, &user_message, 3000, 0.3,
            ).await?;

            log::info!(
                "[YouTube] Chunk {}/{} summarized ({} chars)",
                i + 1, chunks.len(), summary.len()
            );
            chunk_summaries.push(format!("## Section {} of {}\n{}", i + 1, chunks.len(), summary));
        }

        // Final pass: combine chunk summaries into one coherent summary
        let combined = chunk_summaries.join("\n\n");
        let merge_system = "You are a precise summarization assistant. You will receive multiple section summaries from a single YouTube video. Merge them into ONE coherent, comprehensive summary. Preserve all important details, eliminate redundancy, and organize with clear sections. The user will rely on this as a complete representation of the video's content.";
        let merge_message = format!(
            "Merge the following section summaries from the YouTube video \"{}\" into a single comprehensive summary.\n\n---\n{}",
            title, combined
        );

        crate::background::call_llm_oneshot(
            &self.http_client, config, model, merge_system, &merge_message, 4000, 0.3,
        ).await
    }

    /// Split a transcript into chunks of approximately `max_chars` Unicode characters each.
    /// Splits on newline boundaries to avoid cutting mid-line.
    fn split_transcript_chunks<'a>(&self, text: &'a str, max_chars: usize) -> Vec<&'a str> {
        let total_chars = text.chars().count();
        if total_chars <= max_chars {
            return vec![text];
        }

        // Precompute byte offsets for each character boundary
        let char_offsets: Vec<usize> = text
            .char_indices()
            .map(|(i, _)| i)
            .chain(std::iter::once(text.len()))
            .collect();

        let mut chunks = Vec::new();
        let mut start_char = 0;

        while start_char < total_chars {
            let remaining_chars = total_chars - start_char;
            if remaining_chars <= max_chars {
                let byte_start = char_offsets[start_char];
                chunks.push(&text[byte_start..]);
                break;
            }

            let end_char = start_char + max_chars;
            let byte_start = char_offsets[start_char];
            let byte_end = char_offsets[end_char];

            // Try to split on the last newline before the character boundary
            let chunk_slice = &text[byte_start..byte_end];
            let split_byte = if let Some(nl_pos) = chunk_slice.rfind('\n') {
                byte_start + nl_pos + '\n'.len_utf8()
            } else {
                byte_end
            };

            chunks.push(&text[byte_start..split_byte]);

            // Find the char index corresponding to split_byte
            let next_start_char = char_offsets[start_char..]
                .iter()
                .position(|&offset| offset >= split_byte)
                .map(|pos| start_char + pos)
                .unwrap_or(total_chars);
            start_char = next_start_char;
        }

        chunks
    }

    async fn process_gemini_turn<R: Runtime>(
        &self,
        app_handle: &AppHandle<R>,
        config: &crate::config::AppConfig,
        history: &mut Vec<ChatMessage>,
        stream_id: u64,
        selected_model: &str,
        api_key: &str,
        rag_context: Option<&str>,
        is_research_mode: bool,
    ) -> Result<bool, String> {
        let enable_tools = config.enable_tools.unwrap_or(true);
        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:streamGenerateContent",
            selected_model
        );

        // Load memories for injection into system prompt (skip in incognito mode)
        let incognito_mode = config.incognito_mode.unwrap_or(false);
        let memory_context = if incognito_mode {
            None
        } else {
            crate::memories::get_memories_for_prompt(app_handle)
                .ok()
                .filter(|s| !s.is_empty())
        };

        let available_skills = crate::skills::list_available_skills();
        let available_skills_str = if available_skills.is_empty() { None } else { Some(available_skills.join("\n")) };
        let available_skills_opt = available_skills_str.as_deref();

        let session_id = self.session_id.lock().await.clone();
        let mut active_skills_opt: Option<String> = None;
        if let Ok(store) = crate::memories::get_vector_store(app_handle) {
            if let Ok(active_skills) = crate::db::sessions::get_active_skills(&store, &session_id) {
                if !active_skills.is_empty() {
                    let mut active_skills_content = String::new();
                    for skill in active_skills {
                        if let Some(content) = crate::skills::get_skill_content(&skill) {
                            active_skills_content.push_str(&format!("--- SKILL: {} ---\n{}\n\n", skill, content));
                        }
                    }
                    if !active_skills_content.is_empty() {
                        active_skills_opt = Some(active_skills_content);
                    }
                }
            }
        }

        let system_prompt_content = if incognito_mode {
            crate::prompts::get_default_system_prompt(None, None, available_skills_opt, active_skills_opt.as_deref())
        } else if is_research_mode {
            crate::prompts::get_research_system_prompt(available_skills_opt, active_skills_opt.as_deref())
        } else {
            config.system_prompt.clone().unwrap_or_else(|| {
                crate::prompts::get_default_system_prompt(
                    memory_context.as_deref(),
                    rag_context,
                    available_skills_opt,
                    active_skills_opt.as_deref(),
                )
            })
        };

        let contents = construct_gemini_messages(history);
        let system_instruction = Some(GeminiContent {
            role: None,
            parts: vec![GeminiPart::Text {
                text: system_prompt_content.clone(),
            }],
        });

        let session_id_str = self.session_id.lock().await.clone();
        let active_skills_list = crate::memories::get_vector_store(app_handle)
            .and_then(|store| crate::db::sessions::get_active_skills(&store, &session_id_str))
            .unwrap_or_default();

        let gemini_tools = if enable_tools {
            Some(vec![GeminiTool {
                function_declarations: crate::tools::get_all_tools(&active_skills_list)
                    .iter()
                    .map(|t| {
                        // Strip OpenAI-specific fields from parameters
                        let mut params = t.function.parameters.clone();
                        if let Some(obj) = params.as_object_mut() {
                            obj.remove("additionalProperties");
                            obj.remove("strict");
                        }
                        GeminiFunctionDefinition {
                            name: t.function.name.clone(),
                            description: t.function.description.clone(),
                            parameters: params,
                        }
                    })
                    .collect(),
            }])
        } else {
            None
        };

        let supports_thinking = selected_model.contains("2.5")
            || selected_model.contains("gemini-3")
            || selected_model.contains("thinking");

        let request_body = GenerateContentRequest {
            contents,
            tools: gemini_tools,
            system_instruction,
            generation_config: Some(GenerationConfig {
                thinking_config: if supports_thinking {
                    Some(ThinkingConfig {
                        include_thoughts: true,
                        thinking_budget: Some(1024),
                    })
                } else {
                    None
                },
            }),
        };

        let response = self
            .http_client
            .post(&url)
            .header("X-Goog-Api-Key", api_key)
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send()
            .await
            .map_err(|e| format!("API network error: {}", e))?;

        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_default();
            app_handle
                .emit("agent-error", format!("Gemini API Error: {}", error_text))
                .ok();
            return Err(format!("Gemini API Error: {}", error_text));
        }

        use futures_util::StreamExt;
        let mut stream = response.bytes_stream();
        let mut buffer = Vec::new();
        let mut full_text = String::new();
        let mut full_reasoning = String::new();
        let mut tool_calls: Vec<GeminiFunctionCallWithSignature> = Vec::new();

        while let Some(item) = stream.next().await {
            if stream_id == crate::CANCELLED_STREAM_ID.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }

            let chunk = item.map_err(|e| format!("Stream error: {}", e))?;
            buffer.extend_from_slice(&chunk);

            let mut consumed = 0;
            let mut depth = 0;
            let mut in_string = false;
            let mut escape = false;
            let mut start_idx = None;

            for (idx, &b) in buffer.iter().enumerate() {
                let c = b as char;
                if !in_string {
                    if c == '{' {
                        if depth == 0 {
                            start_idx = Some(idx);
                        }
                        depth += 1;
                    } else if c == '}' {
                        depth -= 1;
                        if depth == 0 {
                            if let Some(start) = start_idx {
                                let slice = &buffer[start..=idx];
                                if let Ok(json_obj) =
                                    serde_json::from_slice::<GenerateContentResponse>(slice)
                                {
                                    if let Some(candidates) = json_obj.candidates {
                                        for candidate in candidates {
                                            for part in candidate.content.parts {
                                                let events = parse_gemini_chunk(
                                                    part,
                                                    &mut full_text,
                                                    &mut full_reasoning,
                                                    &mut tool_calls,
                                                );
                                                for event in events {
                                                    match event {
                                                        AgentEvent::ResponseChunk(text) => {
                                                            app_handle
                                                                .emit("agent-response-chunk", text)
                                                                .ok();
                                                        }
                                                        AgentEvent::ReasoningChunk(text) => {
                                                            app_handle
                                                                .emit("agent-reasoning-chunk", text)
                                                                .ok();
                                                        }
                                                        AgentEvent::ToolCall(fc) => {
                                                            let tool_call_event = serde_json::json!({
                                                                "name": fc.function_call.name,
                                                                "args": fc.function_call.args,
                                                                "rawArgs": serde_json::to_string(&fc.function_call.args).unwrap_or_default(),
                                                                "id": format!("call_{}", fc.function_call.name)
                                                            });
                                                            app_handle
                                                                .emit("agent-tool-call", tool_call_event.to_string())
                                                                .ok();
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    consumed = idx + 1;
                                    start_idx = None;
                                }
                            }
                        }
                    }
                }
                if c == '"' && !escape {
                    in_string = !in_string;
                }
                if c == '\\' && !escape {
                    escape = true;
                } else {
                    escape = false;
                }
            }

            if consumed > 0 {
                buffer.drain(0..consumed);
            }
        }

        if !tool_calls.is_empty() {
            let msg = ChatMessage {
                role: "assistant".to_string(),
                content: if full_text.is_empty() {
                    None
                } else {
                    Some(full_text.clone())
                },
                reasoning: if full_reasoning.is_empty() {
                    None
                } else {
                    Some(full_reasoning.trim_end().to_string())
                },
                tool_calls: Some(
                    tool_calls
                        .iter()
                        .enumerate()
                        .map(|(idx, fc)| ToolCall {
                            id: format!("call_{}_{}", fc.function_call.name, idx),
                            tool_type: "function".to_string(),
                            function: FunctionCall {
                                name: fc.function_call.name.clone(),
                                arguments: serde_json::to_string(&fc.function_call.args)
                                    .unwrap_or_default(),
                            },
                            thought_signature: fc.thought_signature.clone(),
                        })
                        .collect(),
                ),
                tool_call_id: None,
                is_cron: None,
                images: None,
            };
            history.push(msg.clone());
            self.insert_single_message_to_db(app_handle, &msg).await;

            for (idx, fc) in tool_calls.into_iter().enumerate() {
                let function_name = &fc.function_call.name;
                let args = &fc.function_call.args;

                // Note: agent-tool-call was already emitted during streaming (with id for dedup).
                // Emitting it again here (without id) caused duplicate cards in the frontend.

                let tool_result = self
                    .execute_tool(app_handle, function_name, args, config)
                    .await;

                let result_payload = serde_json::json!({
                    "name": function_name,
                    "result": tool_result.clone()
                });
                app_handle
                    .emit("agent-tool-result", result_payload.to_string())
                    .ok();

                let msg = ChatMessage {
                    role: "tool".to_string(),
                    content: Some(tool_result),
                    reasoning: None,
                    tool_calls: None,
                    tool_call_id: Some(format!("call_{}_{}", fc.function_call.name, idx)),
                    is_cron: None,
                    images: None,
                };
                history.push(msg.clone());
                self.insert_single_message_to_db(app_handle, &msg).await;
            }
            Ok(true) // Continue loop so model can respond to tool results
        } else {
            let msg = ChatMessage {
                role: "assistant".to_string(),
                content: if full_text.is_empty() {
                    None
                } else {
                    Some(full_text)
                },
                reasoning: if full_reasoning.is_empty() {
                    None
                } else {
                    Some(full_reasoning.trim_end().to_string())
                },
                tool_calls: None,
                tool_call_id: None,
                is_cron: None,
                images: None,
            };
            history.push(msg.clone());
            self.insert_single_message_to_db(app_handle, &msg).await;
            Ok(false) // No tool calls = final response, stop the loop
        }
    }

    async fn process_openrouter_turn<R: Runtime>(
        &self,
        app_handle: &AppHandle<R>,
        config: &crate::config::AppConfig,
        history: &mut Vec<ChatMessage>,
        stream_id: u64,
        rag_context: Option<&str>,
        is_research_mode: bool,
    ) -> Result<bool, String> {
        let selected_model = config
            .selected_model
            .clone()
            .unwrap_or("gemini-2.5-flash-lite".to_string());
        let enable_tools = config.enable_tools.unwrap_or(true);

        // Detect provider from model name and configure accordingly
        let (provider_config, api_key) =
            config.get_model_provider_config(&selected_model, "main chat")?;
        let is_cerebras = provider_config.provider_name == "Cerebras";
        let is_groq = provider_config.provider_name == "Groq";

        let model = provider_config.model_id.clone();
        let reasoning_effort = provider_config.reasoning_effort.clone();
        let provider_name = provider_config.provider_name.clone();
        let url = provider_config.full_url();

        // Load memories for injection into system prompt (skip in incognito mode)
        let incognito_mode = config.incognito_mode.unwrap_or(false);
        let memory_context = if incognito_mode {
            None
        } else {
            crate::memories::get_memories_for_prompt(app_handle)
                .ok()
                .filter(|s| !s.is_empty())
        };

        let available_skills = crate::skills::list_available_skills();
        let available_skills_str = if available_skills.is_empty() { None } else { Some(available_skills.join("\n")) };
        let available_skills_opt = available_skills_str.as_deref();

        let session_id = self.session_id.lock().await.clone();
        let mut active_skills_opt: Option<String> = None;
        if let Ok(store) = crate::memories::get_vector_store(app_handle) {
            if let Ok(active_skills) = crate::db::sessions::get_active_skills(&store, &session_id) {
                if !active_skills.is_empty() {
                    let mut active_skills_content = String::new();
                    for skill in active_skills {
                        if let Some(content) = crate::skills::get_skill_content(&skill) {
                            active_skills_content.push_str(&format!("--- SKILL: {} ---\n{}\n\n", skill, content));
                        }
                    }
                    if !active_skills_content.is_empty() {
                        active_skills_opt = Some(active_skills_content);
                    }
                }
            }
        }

        let system_prompt_content = if incognito_mode {
            crate::prompts::get_default_system_prompt(None, None, available_skills_opt, active_skills_opt.as_deref())
        } else if is_research_mode {
            crate::prompts::get_research_system_prompt(available_skills_opt, active_skills_opt.as_deref())
        } else {
            config.system_prompt.clone().unwrap_or_else(|| {
                crate::prompts::get_default_system_prompt(
                    memory_context.as_deref(),
                    rag_context,
                    available_skills_opt,
                    active_skills_opt.as_deref(),
                )
            })
        };

        let mut messages_with_system = vec![ChatMessage {
            role: "system".to_string(),
            content: Some(system_prompt_content),
            reasoning: None,
            tool_calls: None,
            tool_call_id: None,
            is_cron: None,
            images: None,
        }];
        // 1. Filter out past cron messages from the LLM's context window ONLY during a cron run,
        // so the active cron job focuses on the user's actual conversation. Regular users still see them.
        let is_cron = history.last().and_then(|m| m.is_cron).unwrap_or(false);
        let visible_history: Vec<ChatMessage> = if is_cron {
            let len = history.len();
            let mut visible: Vec<ChatMessage> = Vec::with_capacity(len);
            let mut in_past_cron_segment = false;
            for (i, m) in history.iter().enumerate() {
                // Always keep the very last message (the current cron prompt) in the history.
                if i == len.saturating_sub(1) {
                    visible.push(m.clone());
                    break;
                }
                // A message explicitly marked as cron starts a cron segment (typically a cron user prompt).
                if m.is_cron.unwrap_or(false) {
                    in_past_cron_segment = true;
                    continue;
                }
                // A normal user message (without cron flag) ends any prior cron segment.
                if m.role == "user" {
                    in_past_cron_segment = false;
                }
                if !in_past_cron_segment {
                    visible.push(m.clone());
                }
            }
            visible
        } else {
            history.clone()
        };

        messages_with_system.extend(visible_history);

        let last_idx = messages_with_system.len().saturating_sub(1);
        let multimodal_messages = to_multimodal_messages(
            &messages_with_system
                .into_iter()
                .enumerate()
                .map(|(i, mut msg)| {
                    // If this is a cron job, structurally isolate the current cron prompt from the "chat history"
                    if is_cron && i == last_idx {
                        if let Some(ref mut c) = msg.content {
                            let sanitized = c
                                .replace("<system_directive>", "&lt;system_directive&gt;")
                                .replace("</system_directive>", "&lt;/system_directive&gt;");
                            *c = format!("<system_directive>\nYou are executing a scheduled background task. Please evaluate the user's task instruction strictly against the conversation history preceding this message. Do not consider this directive itself as part of the chat history or summarize it.\nTask: {}\n</system_directive>", sanitized);
                        }
                    }
                    msg
                })
                .collect::<Vec<ChatMessage>>(),
        );

        // Note: multimodal_messages is no longer cloned per request attempt because
        // ChatCompletionRequest is now generic over the messages type, allowing us to borrow.
        // This avoids deep cloning large base64 image data on retry/fallback paths.
        let make_request = |tools_opt: Option<Vec<ToolDefinition>>| {
            let model = model.clone();
            // Borrow messages to avoid cloning
            let messages = multimodal_messages.as_slice();
            let url = url.clone();
            let api_key = api_key.clone();
            let client = self.http_client.clone();
            let use_tools = tools_opt.is_some();
            let reasoning_effort = reasoning_effort.clone();

            async move {
                let request_body = ChatCompletionRequest {
                    model,
                    messages,
                    tools: tools_opt,
                    tool_choice: if use_tools {
                        Some("auto".to_string())
                    } else {
                        None
                    },
                    reasoning_effort,
                    reasoning: None,
                    include_reasoning: if is_cerebras || is_groq {
                        None
                    } else {
                        Some(true)
                    },
                    stream: true,
                };

                client
                    .post(&url)
                    .header("Authorization", format!("Bearer {}", api_key))
                    .header("Content-Type", "application/json")
                    .header("User-Agent", "rust-reqwest/0.12")
                    .json(&request_body)
                    .send()
                    .await
            }
        };

        let session_id_str = self.session_id.lock().await.clone();
        let active_skills_list = crate::memories::get_vector_store(app_handle)
            .and_then(|store| crate::db::sessions::get_active_skills(&store, &session_id_str))
            .unwrap_or_default();

        let is_olmo_think = model.contains("olmo-3.1-32b-think");
        let is_strict_blacklisted = model.to_lowercase().contains("upstage");

        let current_tools = if enable_tools && !is_olmo_think {
            Some(
                crate::tools::get_all_tools(&active_skills_list)
                    .iter()
                    .map(|t| ToolDefinition {
                        tool_type: t.tool_type.clone(),
                        function: FunctionDefinition {
                            name: t.function.name.clone(),
                            description: t.function.description.clone(),
                            parameters: t.function.parameters.clone(),
                            strict: if is_strict_blacklisted { None } else { t.function.strict },
                        },
                    })
                    .collect(),
            )
        } else {
            None
        };

        let mut response = make_request(current_tools.clone())
            .await
            .map_err(|e| format!("{} network error: {}", provider_name, e))?;

        if response.status() == 404 && enable_tools {
            println!(
                "[{}] Got 404 with tools, retrying without tools...",
                provider_name
            );
            response = make_request(None)
                .await
                .map_err(|e| format!("{} network error (retry): {}", provider_name, e))?;
        }

        // Check for token quota errors on Cerebras/Groq and fallback to OpenRouter
        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_default();
            let is_quota_error = error_text.contains("token_quota_exceeded")
                || error_text.contains("too_many_tokens")
                || error_text.contains("rate_limit")
                || error_text.contains("tokens per minute");

            // Only fallback for Cerebras/Groq quota errors, not OpenRouter
            if is_quota_error && (is_cerebras || is_groq) {
                // Check if OpenRouter is available for fallback
                if let Some(openrouter_key) = &config.openrouter_api_key {
                    // Emit fallback notification with original error
                    let fallback_event = serde_json::json!({
                        "title": "API Error: Moving to OpenRouter",
                        "details": format!("{} error: {}", provider_name, error_text)
                    });
                    app_handle
                        .emit("agent-fallback", fallback_event.to_string())
                        .ok();

                    // Rebuild request for OpenRouter
                    let openrouter_url = "https://openrouter.ai/api/v1/chat/completions";
                    // Use configured fallback model or default
                    let fallback_model = config
                        .fallback_model
                        .clone()
                        .unwrap_or_else(|| "openai/gpt-oss-120b:free".to_string());

                    let fallback_body = ChatCompletionRequest {
                        model: fallback_model,
                        messages: multimodal_messages.as_slice(),
                        tools: current_tools.clone(),
                        tool_choice: if current_tools.is_some() {
                            Some("auto".to_string())
                        } else {
                            None
                        },
                        reasoning_effort: None,
                        reasoning: None,
                        include_reasoning: Some(true),
                        stream: true,
                    };

                    response = self
                        .http_client
                        .post(openrouter_url)
                        .header("Authorization", format!("Bearer {}", openrouter_key))
                        .header("Content-Type", "application/json")
                        .header("User-Agent", "rust-reqwest/0.12")
                        .json(&fallback_body)
                        .send()
                        .await
                        .map_err(|e| format!("OpenRouter fallback network error: {}", e))?;

                    // Check if fallback succeeded
                    if !response.status().is_success() {
                        let fallback_error = response.text().await.unwrap_or_default();
                        app_handle
                            .emit(
                                "agent-error",
                                format!("OpenRouter fallback error: {}", fallback_error),
                            )
                            .ok();
                        return Err(format!("OpenRouter fallback error: {}", fallback_error));
                    }
                    // Continue with fallback response
                } else {
                    // No OpenRouter key available, show original error
                    app_handle
                        .emit(
                            "agent-error",
                            format!("{} error: {}", provider_name, error_text),
                        )
                        .ok();
                    return Err(format!("{} error: {}", provider_name, error_text));
                }
            } else {
                // Not a quota error or already on OpenRouter, show original error
                app_handle
                    .emit(
                        "agent-error",
                        format!("{} error: {}", provider_name, error_text),
                    )
                    .ok();
                return Err(format!("{} error: {}", provider_name, error_text));
            }
        }

        let mut full_content = String::new();
        let mut full_reasoning = String::new();
        let mut tool_calls_buffer: Vec<ToolCall> = Vec::new();
        use futures_util::StreamExt;

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();

        while let Some(item) = stream.next().await {
            if stream_id == crate::CANCELLED_STREAM_ID.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
            let chunk = item.map_err(|e| {
                log::debug!("Stream chunk error: {}", e);
                format!("Stream error: {}", e)
            })?;
            let chunk_str = String::from_utf8_lossy(&chunk);
            buffer.push_str(&chunk_str);

            let mut consumed = 0;
            if let Some(last_newline) = buffer.rfind('\n') {
                let content_to_process = &buffer[..last_newline];
                for line in content_to_process.lines() {
                    let line = line.trim();
                    if line.starts_with("data: ") {
                        let json_str = &line[6..];
                        if json_str == "[DONE]" {
                            continue;
                        }

                        if let Ok(json) = serde_json::from_str::<serde_json::Value>(json_str) {
                            if let Some(choices) = json.get("choices").and_then(|c| c.as_array()) {
                                if let Some(choice) = choices.first() {
                                    if let Some(reasoning) = choice["delta"].get("reasoning") {
                                        if !reasoning.is_null() && reasoning.as_str().is_some() {
                                            let reasoning_str = reasoning.as_str().unwrap();
                                            full_reasoning.push_str(reasoning_str);
                                            app_handle
                                                .emit("agent-reasoning-chunk", reasoning_str)
                                                .ok();
                                        }
                                    }

                                    if let Some(content) =
                                        choice["delta"].get("content").and_then(|c| c.as_str())
                                    {
                                        full_content.push_str(content);
                                        app_handle.emit("agent-response-chunk", content).ok();
                                    }

                                                if let Some(delta_tool_calls) =
                                                    choice["delta"].get("tool_calls")
                                                {
                                                    if let Some(tool_calls_arr) = delta_tool_calls.as_array() {
                                                        for tool_call_json in tool_calls_arr {
                                                            let index =
                                                                tool_call_json["index"].as_u64().unwrap_or(0)
                                                                    as usize;
                                                            if index >= tool_calls_buffer.len() {
                                                                tool_calls_buffer.resize(
                                                                    index + 1,
                                                                    ToolCall {
                                                                        id: String::new(),
                                                                        tool_type: "function".to_string(),
                                                                        function: FunctionCall {
                                                                            name: String::new(),
                                                                            arguments: String::new(),
                                                                        },
                                                                        thought_signature: None,
                                                                    },
                                                                );
                                                            }
                                                            let target = &mut tool_calls_buffer[index];
                                                            if let Some(id) = tool_call_json["id"].as_str() {
                                                                target.id = id.to_string();
                                                            }
                                                            if let Some(func) = tool_call_json.get("function") {
                                                                if let Some(name) = func["name"].as_str() {
                                                                    target.function.name.push_str(name);
                                                                }
                                                                if let Some(args) = func["arguments"].as_str() {
                                                                    target.function.arguments.push_str(args);
                                                                }
                                                            }

                                                            // Emit tool call update for real-time UI mapping
                                                            if !target.function.name.is_empty() {
                                                                let args_json: serde_json::Value = serde_json::from_str(&target.function.arguments).unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
                                                                let event_payload = serde_json::json!({
                                                                    "name": target.function.name,
                                                                    "args": args_json,
                                                                    "rawArgs": target.function.arguments,
                                                                    "id": target.id
                                                                });
                                                                app_handle.emit("agent-tool-call", event_payload.to_string()).ok();
                                                            }
                                                        }
                                                    }
                                                }
                                }
                            }
                        }
                    }
                }
                consumed = last_newline + 1;
            }

            if consumed > 0 {
                buffer.drain(0..consumed);
            }
        }

        if !full_content.is_empty() || !tool_calls_buffer.is_empty() || !full_reasoning.is_empty() {
            let msg = ChatMessage {
                role: "assistant".to_string(),
                content: if full_content.is_empty() {
                    None
                } else {
                    Some(full_content.clone())
                },
                reasoning: if full_reasoning.is_empty() {
                    None
                } else {
                    Some(full_reasoning.clone())
                },
                tool_calls: if tool_calls_buffer.is_empty() {
                    None
                } else {
                    Some(tool_calls_buffer.clone())
                },
                tool_call_id: None,
                is_cron: None,
                images: None,
            };
            history.push(msg.clone());
            self.insert_single_message_to_db(app_handle, &msg).await;

            if !tool_calls_buffer.is_empty() {
                for tool_call in &tool_calls_buffer {
                    let function_name = &tool_call.function.name;
                    let arguments = &tool_call.function.arguments;
                    let args: Value = serde_json::from_str(arguments).unwrap_or(json!({}));

                    // Note: agent-tool-call was already emitted during streaming (line ~2259, with id for dedup).
                    // A second emit here duplicated the card in the frontend.

                    let tool_result = self
                        .execute_tool(app_handle, function_name, &args, config)
                        .await;

                    let result_payload = serde_json::json!({
                        "name": function_name,
                        "result": tool_result.clone()
                    });
                    app_handle
                        .emit("agent-tool-result", result_payload.to_string())
                        .ok();

                    let msg = ChatMessage {
                        role: "tool".to_string(),
                        content: Some(tool_result),
                        reasoning: None,
                        tool_calls: None,
                        tool_call_id: Some(tool_call.id.clone()),
                        is_cron: None,
                        images: None,
                    };
                    history.push(msg.clone());
                    self.insert_single_message_to_db(app_handle, &msg).await;
                }
                Ok(true) // Continue loop so model can respond to tool results
            } else {
                Ok(false) // No tool calls = final response, stop the loop
            }
        } else {
            Ok(false) // No content = stop
        }
    }
}
