//! Chat history mutation, session rotation, and SQLite persistence helpers
//! attached to the [`Agent`] type.
//!
//! Every method here keeps `self.history` / `self.session_id` /
//! `self.last_archived_hash` / `self.backup_history` consistent. None of the
//! methods touch the network — that work belongs in `turns/` and the
//! provider helpers.

use tauri::AppHandle;

use super::hash::calculate_history_hash;
use super::types::ChatMessage;
use super::Agent;

impl<R: tauri::Runtime> Agent<R> {
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
            let current_hash = calculate_history_hash(&history_clone);
            let should_archive = current_hash != *last_hash_guard;
            let current_session_id = session_id_guard.clone();

            (history_clone, current_session_id, should_archive)
        };

        // Phase 2: Archive on clear if changes occurred.
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
                calculate_history_hash(&history_clone)
            } else {
                0
            };
        }

        let new_session_id = uuid::Uuid::new_v4().to_string();
        {
            let mut session_id_guard = self.session_id.lock().await;
            *session_id_guard = new_session_id.clone();
        }

        // Initialize the new session in the DB immediately so FK constraints pass.
        if let Ok(store) = crate::memories::get_vector_store(&self.app_handle) {
            let now = chrono::Utc::now().to_rfc3339();
            let session = crate::db::sessions::SessionRow {
                id: new_session_id.clone(),
                title: "Active Session".to_string(),
                summary: None,
                created_at: now.clone(),
                updated_at: now.clone(),
                active_personas: Some("[]".to_string()),
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
                    if let Some(file_name) = uri.rsplit('/').next() {
                        let delete_url = format!(
                            "{}/{}",
                            crate::endpoints::gemini_files_base(),
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
            let new_hash = calculate_history_hash(&saved_history);

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

    pub async fn load_session_from_db(
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
                    for msg in msg_iter.flatten() {
                        history.push(msg);
                    }
                }

                history
            } else {
                return Err("Failed to open database".to_string());
            }
        };

        let hash = calculate_history_hash(&new_history);
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

    pub async fn persist_history(&self) {
        let history = self.history.lock().await;
        let session_id = self.session_id.lock().await;

        if let Ok(store) = crate::memories::get_vector_store(&self.app_handle) {
            // Delete existing messages for session.
            let _ = store.conn.execute(
                "DELETE FROM messages WHERE session_id = ?",
                rusqlite::params![*session_id],
            );

            // Insert current history.
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

            // Update session updated_at.
            let _ = store.conn.execute(
                "UPDATE sessions SET updated_at = ? WHERE id = ?",
                rusqlite::params![chrono::Utc::now().to_rfc3339(), *session_id],
            );
        }
    }

    pub(crate) async fn insert_single_message_to_db(
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
}


