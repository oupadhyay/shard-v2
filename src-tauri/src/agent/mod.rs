/**
 * Agent module - AI chat agent with Gemini and OpenRouter support.
 *
 * The agent's behaviour is split across small, single-responsibility
 * modules. This file is intentionally thin: it owns only the [`Agent`]
 * struct, the per-turn [`TurnContext`], and `Agent::new`. Every method on
 * `impl<R: tauri::Runtime> Agent<R>` lives in one of the submodules below.
 *
 * Module layout:
 *   - [`gemini`]       — Interactions API request/response codecs.
 *   - [`openrouter`]   — OpenAI-compatible request helpers + `to_multimodal_messages`.
 *   - [`schema`]       — `normalize_gemini_schema` (proto-friendly JSON Schema).
 *   - [`hash`]         — `calculate_history_hash`.
 *   - [`state`]        — history mutators + SQLite session persistence.
 *   - [`retry`]        — frontend-triggered KaTeX retry path.
 *   - [`youtube_summary`] — long-transcript summarization (chunked).
 *   - [`research`]     — Gemini-based intent classifier for research mode.
 *   - [`tools`]        — `execute_tool` cache wrapper + tool dispatch.
 *   - [`turns`]        — per-turn streaming handlers (Gemini + OpenRouter).
 *   - [`process`]      — `process_message` orchestrator.
 */
mod gemini;
mod hash;
pub(crate) mod openrouter;
mod process;
mod research;
mod retry;
mod schema;
mod state;
mod tools;
mod turns;
mod types;
mod youtube_summary;

pub use gemini::{
    construct_gemini_messages, construct_interactions_input, parse_gemini_chunk,
    parse_interactions_sse_line, process_interactions_event, AgentEvent,
};
pub use openrouter::{has_images, supports_tools, to_multimodal_messages};
pub use types::*;

// Phase 6 refactor — re-export the pure helpers so existing callers and
// tests reach them under the same `crate::agent::xxx` paths they used
// before the split.
pub(crate) use schema::normalize_gemini_schema;
#[cfg(test)]
pub(crate) use hash::calculate_history_hash;
#[cfg(test)]
pub(crate) use youtube_summary::split_transcript_chunks;

use reqwest::Client;
use tauri::Manager;
use tokio::sync::Mutex;

/// Context passed into each LLM turn (RAG, peer info, mode flags).
#[derive(Default)]
pub(crate) struct TurnContext<'a> {
    pub rag_context: Option<&'a str>,
    pub peer_card: Option<&'a str>,
    pub peer_representation: Option<&'a str>,
    pub is_research_mode: bool,
}

/// The main AI Agent managing chat history and API interactions.
///
/// Generic over the Tauri runtime with a `Wry` default so existing call sites
/// (`Arc<Agent>`) compile unchanged. The eval harness substitutes
/// `MockRuntime` to drive the agent headlessly.
pub struct Agent<R: tauri::Runtime = tauri::Wry> {
    pub(crate) history: Mutex<Vec<ChatMessage>>,
    pub(crate) http_client: Client,
    pub(crate) uploaded_files: Mutex<Vec<String>>,
    pub(crate) backup_history: Mutex<Option<(Vec<ChatMessage>, String)>>,
    pub session_id: Mutex<String>,
    pub last_archived_hash: Mutex<u64>,
    pub app_handle: tauri::AppHandle<R>,
}

impl<R: tauri::Runtime> Agent<R> {
    pub fn new(app_handle: tauri::AppHandle<R>) -> Self {
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

                log::info!(
                    "Loaded {} messages from SQLite for session [redacted]",
                    history.len(),
                );
            } else {
                let now = chrono::Utc::now().to_rfc3339();
                let session = crate::db::sessions::SessionRow {
                    id: session_id.clone(),
                    title: "Active Session".to_string(),
                    summary: None,
                    created_at: now.clone(),
                    updated_at: now.clone(),
                    active_personas: Some("[]".to_string()),
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
}
