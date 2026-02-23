use crate::agent::{ChatMessage, PersistedChatState};
use crate::vector_store::VectorStore;
use chrono::Utc;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::fs;
use tauri::{AppHandle, Manager};

#[derive(Debug, Serialize, Deserialize)]
pub struct SessionRow {
    pub id: String,
    pub title: String,
    pub summary: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MessageRow {
    pub id: String,
    pub session_id: String,
    pub role: String,
    pub content: String,
    pub created_at: String,
}

/// Insert a new session or update an existing one
pub fn insert_session(store: &VectorStore, session: &SessionRow) -> Result<(), String> {
    store
        .conn
        .execute(
            "INSERT INTO sessions (id, title, summary, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT(id) DO UPDATE SET title = excluded.title, summary = excluded.summary, updated_at = excluded.updated_at",
            params![
                session.id,
                session.title,
                session.summary,
                session.created_at,
                session.updated_at
            ],
        )
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Insert a new message or update an existing one
pub fn insert_message(store: &VectorStore, msg: &MessageRow) -> Result<(), String> {
    store
        .conn
        .execute(
            "INSERT OR REPLACE INTO messages (id, session_id, role, content, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                msg.id,
                msg.session_id,
                msg.role,
                msg.content,
                msg.created_at
            ],
        )
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Helper: Check if migration has run
fn is_migration_completed(store: &VectorStore) -> Result<bool, String> {
    let result: Option<String> = store
        .conn
        .query_row(
            "SELECT value FROM metadata WHERE key = 'session_migration_completed'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e: rusqlite::Error| e.to_string())?;

    Ok(result == Some("true".to_string()))
}

/// Helper: Mark migration as completed
fn mark_migration_completed(store: &VectorStore) -> Result<(), String> {
    store
        .conn
        .execute(
            "INSERT OR REPLACE INTO metadata (key, value) VALUES ('session_migration_completed', 'true')",
            [],
        )
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Run the migration from legacy JSON/Markdown to SQLite
pub fn run_migration(app_handle: &AppHandle, store: &VectorStore) -> Result<(), String> {
    if is_migration_completed(store)? {
        log::info!("[db::sessions] Migration already completed, skipping.");
        return Ok(());
    }

    log::info!("[db::sessions] Starting legacy session migration to SQLite...");

    let app_data_dir = app_handle
        .path()
        .app_data_dir()
        .map_err(|_| "Failed to get app_data_dir".to_string())?;

    // 1. Migrate active chat from chat_history.json
    let history_path = app_data_dir.join("chat_history.json");
    if history_path.exists() {
        if let Ok(contents) = fs::read_to_string(&history_path) {
            log::info!("[db::sessions] Found chat_history.json, parsing...");
            let (history, mut session_id) = if let Ok(state) = serde_json::from_str::<PersistedChatState>(&contents) {
                (state.history, state.session_id)
            } else if let Ok(msgs) = serde_json::from_str::<Vec<ChatMessage>>(&contents) {
                (msgs, uuid::Uuid::new_v4().to_string())
            } else {
                (Vec::<ChatMessage>::new(), uuid::Uuid::new_v4().to_string())
            };

            if !history.is_empty() {
                if session_id.is_empty() {
                    session_id = uuid::Uuid::new_v4().to_string();
                }

                let now = Utc::now().to_rfc3339();
                let session = SessionRow {
                    id: session_id.clone(),
                    title: "Active Session".to_string(),
                    summary: None,
                    created_at: now.clone(),
                    updated_at: now.clone(),
                };

                if let Err(e) = insert_session(store, &session) {
                    log::warn!("[db::sessions] Failed to insert active session: {}", e);
                } else {
                    for chat_msg in history {
                        let msg_row = MessageRow {
                            id: uuid::Uuid::new_v4().to_string(),
                            session_id: session_id.clone(),
                            role: chat_msg.role.clone(),
                            content: serde_json::to_string(&chat_msg).unwrap_or_else(|_| "{}".to_string()),
                            created_at: now.clone(), // We don't have per-message timestamps in the legacy schema
                        };
                        let _ = insert_message(store, &msg_row);
                    }
                    log::info!("[db::sessions] Migrated active session: {}", session_id);
                }
            }
        }
    }

    // 2. Migrate archived sessions from memories/sessions/*.md
    if let Ok(memory_dir) = crate::memories::get_memory_transcripts_dir(app_handle) {
        if let Ok(entries) = fs::read_dir(&memory_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("md") {
                    if let Some(filename) = path.file_stem().and_then(|s| s.to_str()) {
                        log::info!("[db::sessions] Migrating archived file: {}", filename);
                        // Pattern: YYYY-MM-DD-slug-UUID
                        // Let's extract the UUID from the end if possible
                        let mut parts: Vec<&str> = filename.split('-').collect();
                        let mut session_id = uuid::Uuid::new_v4().to_string(); // fallback

                        // UUIDs are typically 36 chars long (with hyphens), but here it might be split
                        // So finding a strict UUID might be tough if it's split.
                        // Wait, UUIDs have hyphens, so they split into 5 parts.
                        if parts.len() >= 5 {
                            let possible_uuid = parts[parts.len()-5..].join("-");
                            if uuid::Uuid::parse_str(&possible_uuid).is_ok() {
                                session_id = possible_uuid;
                                parts.truncate(parts.len() - 5);
                            } else {
                                // Maybe the UUID has no hyphens or something else?
                                // We'll just generate one if parsing fails.
                            }
                        }

                        // Construct a title from the remaining parts
                        let mut title = if parts.len() > 3 {
                            parts[3..].join(" ") // skipping YYYY-MM-DD
                        } else {
                            "Archived Session".to_string()
                        };

                        if title.is_empty() {
                            title = "Archived Session".to_string();
                        }

                        let content = fs::read_to_string(&path).unwrap_or_default();

                        // Extract summary if present
                        let mut summary = None;
                        if let Some(summary_idx) = content.find("## Summary\n\n") {
                            let start = summary_idx + 12;
                            if let Some(end_idx) = content[start..].find("\n\n---") {
                                summary = Some(content[start..start+end_idx].trim().to_string());
                            }
                        }

                        let modified_time = fs::metadata(&path)
                            .and_then(|m| m.modified())
                            .map(|m| chrono::DateTime::<Utc>::from(m).to_rfc3339())
                            .unwrap_or_else(|_| Utc::now().to_rfc3339());

                        let session = SessionRow {
                            id: session_id.clone(),
                            title,
                            summary,
                            created_at: modified_time.clone(),
                            updated_at: modified_time.clone(),
                        };

                        if insert_session(store, &session).is_ok() {
                            // Insert a single summary message containing the whole transcript to preserve context
                            let dummy_chat = ChatMessage {
                                role: "assistant".to_string(),
                                content: Some(content.clone()),
                                reasoning: None,
                                tool_calls: None,
                                tool_call_id: None,
                                images: None,
                            };

                            let msg_row = MessageRow {
                                id: uuid::Uuid::new_v4().to_string(),
                                session_id: session_id.clone(),
                                role: "assistant".to_string(),
                                content: serde_json::to_string(&dummy_chat).unwrap_or_else(|_| "{}".to_string()),
                                created_at: modified_time.clone(),
                            };
                            let _ = insert_message(store, &msg_row);
                        }
                    }
                }
            }
        }
    }

    // Mark as completed
    mark_migration_completed(store)?;
    log::info!("[db::sessions] Session migration complete.");

    Ok(())
}

pub fn search_sessions_by_time(
    store: &VectorStore,
    query: &str,
    time_filter: &str,
    limit: usize,
) -> Result<String, String> {
    let mut sql = "SELECT s.id, s.title, s.summary, s.updated_at FROM sessions s LEFT JOIN messages m ON s.id = m.session_id".to_string();
    let mut conditions = Vec::new();
    let mut params: Vec<String> = Vec::new();

    // Map time_filter to date condition
    let now = chrono::Utc::now();
    match time_filter.to_lowercase().as_str() {
        "yesterday" => {
            let start = now - chrono::Duration::days(1);
            let end = now;
            conditions.push(format!("s.updated_at >= '{}' AND s.updated_at < '{}'", start.format("%Y-%m-%d"), end.format("%Y-%m-%d")));
        }
        "last_week" => {
            let start = now - chrono::Duration::days(7);
            conditions.push(format!("s.updated_at >= '{}'", start.format("%Y-%m-%d")));
        }
        "last_conversation" => {
            // order by handled by main query
        }
        specific_date => {
            if specific_date.len() == 10 && specific_date.chars().filter(|c| *c == '-').count() == 2 {
                conditions.push(format!("s.updated_at LIKE '{}%'", specific_date));
            }
        }
    }

    if !query.is_empty() && query != "*" {
        conditions.push("(s.title LIKE ? OR s.summary LIKE ?)".to_string());
        let like_query = format!("%{}%", query);
        params.push(like_query.clone());
        params.push(like_query);
    }

    if !conditions.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&conditions.join(" AND "));
    }

    sql.push_str(" GROUP BY s.id HAVING COUNT(m.id) > 0");
    sql.push_str(" ORDER BY s.updated_at DESC");
    if limit > 0 {
        sql.push_str(&format!(" LIMIT {}", limit));
    }

    let mut stmt = store.conn.prepare(&sql).map_err(|e| e.to_string())?;

    let mut rows = if params.is_empty() {
        stmt.query([]).map_err(|e| e.to_string())?
    } else if params.len() == 2 {
        stmt.query([&params[0], &params[1]]).map_err(|e| e.to_string())?
    } else {
        return Err("Unexpected parameter count".to_string());
    };

    let mut results = Vec::new();
    while let Ok(Some(row)) = rows.next() {
        let id: String = row.get(0).unwrap_or_default();
        let title: String = row.get(1).unwrap_or_default();
        let summary: Option<String> = row.get(2).unwrap_or_default();
        let updated_at: String = row.get(3).unwrap_or_default();

        results.push(serde_json::json!({
            "session_id": id,
            "title": title,
            "summary": summary.unwrap_or_else(|| "No summary available".to_string()),
            "date": updated_at
        }));
    }

    if results.is_empty() {
        Ok("No matching sessions found.".to_string())
    } else {
        serde_json::to_string_pretty(&results).map_err(|e| e.to_string())
    }
}

/// Parses legacy Markdown transcript files (migrated) back into a vector of ChatMessage
pub fn parse_legacy_markdown_transcript(content: &str) -> Vec<ChatMessage> {
    let mut messages = Vec::new();
    let mut current_role = String::new();
    let mut current_text = String::new();
    let mut current_reasoning = String::new();
    let mut current_tool_calls: Vec<crate::agent::ToolCall> = Vec::new();

    let lines: Vec<&str> = content.lines().collect();
    let mut in_dialogue = false;
    let mut in_thought_details = false;
    let mut in_thought_blockquote = false;
    let mut in_tool_call = false;
    let mut current_tool_name = String::new();

    for line in lines {
        if line == "## Dialogue" {
            in_dialogue = true;
            continue;
        }
        if !in_dialogue {
            continue;
        }

        if line.starts_with("### ") {
            if !current_role.is_empty() {
                messages.push(ChatMessage {
                    role: current_role.clone(),
                    content: if current_text.trim().is_empty() { None } else { Some(current_text.trim().to_string()) },
                    reasoning: if current_reasoning.trim().is_empty() { None } else { Some(current_reasoning.trim().to_string()) },
                    tool_calls: if current_tool_calls.is_empty() { None } else { Some(current_tool_calls.clone()) },
                    tool_call_id: None,
                    images: None,
                });
                current_text.clear();
                current_reasoning.clear();
                current_tool_calls.clear();
            }

            if line.starts_with("### User") {
                current_role = "user".to_string();
            } else if line.starts_with("### Assistant") {
                current_role = "assistant".to_string();
            } else {
                current_role = "tool".to_string();
            }
        } else if line.starts_with("<details class=\"thinking-accordion\">") || line.starts_with("<details><summary>Thought Process") {
            in_thought_details = true;
        } else if line.starts_with("*Thought process:*") || line.starts_with("_Thought process:_") {
            in_thought_blockquote = true;
        } else if line.starts_with("*Tool Call:* `") || line.starts_with("_Tool Call:_ `") {
            let mut parsed_tool_name = String::new();
            if let Some(start_idx) = line.find('`') {
                if let Some(end_idx) = line[start_idx + 1..].find('`') {
                    parsed_tool_name = line[start_idx + 1..start_idx + 1 + end_idx].to_string();
                }
            }
            if parsed_tool_name.is_empty() {
                parsed_tool_name = "unknown_tool".to_string();
            }
            current_tool_calls.push(crate::agent::ToolCall {
                id: uuid::Uuid::new_v4().to_string(), // Fake ID
                tool_type: "function".to_string(),
                function: crate::agent::FunctionCall {
                    name: parsed_tool_name,
                    arguments: "{}".to_string(), // We don't have the args cleanly
                },
                thought_signature: Some("".to_string()),
            });
        } else if line.starts_with("<details class=\"tool-accordion\">") || line.starts_with("<details><summary>🛠️ Tool Call: ") {
            in_tool_call = true;
            // Extract tool name if possible
            if let Some(idx) = line.find("🛠️ Tool Call: ") {
                let start = idx + 15; // length of "🛠️ Tool Call: " is 16 chars visually, byte len is ~18. Wait, just split.
                if let Some(end) = line[start..].find("</summary>") {
                    current_tool_name = line[start..start+end].trim().to_string();
                } else if let Some(end) = line[start..].find("`") { // fallback for `tool`
                    current_tool_name = line[start..start+end].trim().to_string();
                }
            } else {
                current_tool_name = "unknown_tool".to_string();
            }
        } else if line.starts_with("</details>") {
            if in_thought_details {
                in_thought_details = false;
            } else if in_tool_call {
                in_tool_call = false;
                // We fake a tool call block
                current_tool_calls.push(crate::agent::ToolCall {
                    id: uuid::Uuid::new_v4().to_string(), // Fake ID
                    tool_type: "function".to_string(),
                    function: crate::agent::FunctionCall {
                        name: if current_tool_name.is_empty() { "tool".to_string() } else { current_tool_name.clone() },
                        arguments: "{}".to_string(), // We don't have the explicit args cleanly, just the block text
                    },
                    thought_signature: Some("".to_string()),
                });
                current_tool_name.clear();
            } else {
                // stray closing tag
                current_text.push_str(line);
                current_text.push('\n');
            }
        } else {
            if !current_role.is_empty() {
                if in_thought_details {
                    // Skip the <summary> line itself if it's separate
                    if !line.starts_with("<summary>") {
                        current_reasoning.push_str(line);
                        current_reasoning.push('\n');
                    }
                } else if in_thought_blockquote {
                    if line.starts_with(">") {
                        let content = line.trim_start_matches('>').trim_start_matches(' ');
                        current_reasoning.push_str(content);
                        current_reasoning.push('\n');
                    } else if line.trim().is_empty() {
                        current_reasoning.push('\n');
                    } else {
                        // Non-empty line without '>', exits blockquote thought
                        in_thought_blockquote = false;
                        current_text.push_str(line);
                        current_text.push('\n');
                    }
                } else if in_tool_call {
                    // we SHOULD convert them to actual `tool_calls`.
                    current_text.push_str(&format!("`{}`\n", line)); // Fallback inner content
                } else {
                    current_text.push_str(line);
                    current_text.push('\n');
                }
            }
        }
    }

    if !current_role.is_empty() && (!current_text.trim().is_empty() || !current_reasoning.is_empty() || !current_tool_calls.is_empty()) {
        messages.push(ChatMessage {
            role: current_role,
            content: if current_text.trim().is_empty() { None } else { Some(current_text.trim().to_string()) },
            reasoning: if current_reasoning.trim().is_empty() { None } else { Some(current_reasoning.trim().to_string()) },
            tool_calls: if current_tool_calls.is_empty() { None } else { Some(current_tool_calls) },
            tool_call_id: None,
            images: None,
        });
    }

    messages
}

pub fn get_session_transcript(store: &VectorStore, session_id: &str) -> Result<String, String> {
    let mut stmt = store.conn.prepare("SELECT role, content FROM messages WHERE session_id = ? ORDER BY created_at ASC").map_err(|e| e.to_string())?;
    let mut rows = stmt.query([session_id]).map_err(|e| e.to_string())?;

    let mut transcript = String::new();
    while let Ok(Some(row)) = rows.next() {
        let role: String = row.get(0).unwrap_or_default();
        let content_json: String = row.get(1).unwrap_or_default();

        let msg_text = if let Ok(chat_msg) = serde_json::from_str::<ChatMessage>(&content_json) {
            chat_msg.content.unwrap_or_default()
        } else {
            content_json
        };

        if !msg_text.is_empty() {
            transcript.push_str(&format!("{}: {}\n\n", role.to_uppercase(), msg_text));
        }
    }

    if transcript.is_empty() {
        return Err(format!("Session {} not found or has no messages.", session_id));
    }

    Ok(transcript.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_legacy_thought_process_parsing() {
        let content = "## Dialogue\n\n### Assistant\nHello.\n\n*Thought process:*\n\n> This is a test.\n> Second line.\n\n*Tool Call:* `web_search`\n\nNormal text.";
        let messages = parse_legacy_markdown_transcript(content);
        let msg = &messages[0];
        assert_eq!(msg.content.as_deref().unwrap(), "Hello.\n\nNormal text.");
        assert_eq!(msg.reasoning.as_deref().unwrap(), "This is a test.\nSecond line.");
        assert_eq!(msg.tool_calls.as_ref().unwrap().len(), 1);
        assert_eq!(msg.tool_calls.as_ref().unwrap()[0].function.name, "web_search");
    }
}
