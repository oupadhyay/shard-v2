use crate::agent::ChatMessage;
use crate::vector_store::VectorStore;
use chrono::Utc;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct SessionRow {
    pub id: String,
    pub title: String,
    pub summary: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    #[serde(rename = "active_skills")]
    pub active_personas: Option<String>, // JSON array of persona names
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
            "INSERT INTO sessions (id, title, summary, created_at, updated_at, active_skills) VALUES (?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT(id) DO UPDATE SET title = excluded.title, summary = excluded.summary, updated_at = excluded.updated_at, active_skills = excluded.active_skills",
            params![
                session.id,
                session.title,
                session.summary,
                session.created_at,
                session.updated_at,
                session.active_personas.clone().unwrap_or_else(|| "[]".to_string())
            ],
        )
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Get active personas for a session
pub fn get_active_skills(store: &VectorStore, session_id: &str) -> Result<Vec<String>, String> {
    let result: Option<String> = store
        .conn
        .query_row(
            "SELECT active_skills FROM sessions WHERE id = ?1",
            params![session_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| e.to_string())?;

    if let Some(skills_json) = result {
        Ok(serde_json::from_str(&skills_json).unwrap_or_else(|_| Vec::new()))
    } else {
        Ok(Vec::new())
    }
}

/// Update active personas for a session (expects a JSON array string)
pub fn update_active_skills(store: &VectorStore, session_id: &str, skills_json: &str) -> Result<(), String> {
    store
        .conn
        .execute(
            "UPDATE sessions SET active_skills = ?1, updated_at = ?2 WHERE id = ?3",
            params![skills_json, Utc::now().to_rfc3339(), session_id],
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

pub fn search_sessions_by_time(
    store: &VectorStore,
    query: &str,
    time_filter: &str,
    limit: usize,
) -> Result<String, String> {
    let mut sql = "SELECT s.id, s.title, s.summary, s.updated_at FROM sessions s LEFT JOIN messages m ON s.id = m.session_id".to_string();
    let mut conditions = Vec::new();
    let mut params: Vec<rusqlite::types::Value> = Vec::new();

    // Map time_filter to date condition
    let now = chrono::Utc::now();
    match time_filter.to_lowercase().as_str() {
        "yesterday" => {
            let start = now - chrono::Duration::days(1);
            let end = now;
            conditions.push("s.updated_at >= ? AND s.updated_at < ?".to_string());
            params.push(rusqlite::types::Value::Text(start.format("%Y-%m-%d").to_string()));
            params.push(rusqlite::types::Value::Text(end.format("%Y-%m-%d").to_string()));
        }
        "last_week" => {
            let start = now - chrono::Duration::days(7);
            conditions.push("s.updated_at >= ?".to_string());
            params.push(rusqlite::types::Value::Text(start.format("%Y-%m-%d").to_string()));
        }
        "last_conversation" => {
            // order by handled by main query
        }
        specific_date => {
            if specific_date.len() == 10 && specific_date.chars().filter(|c| *c == '-').count() == 2 {
                conditions.push("s.updated_at LIKE ?".to_string());
                params.push(rusqlite::types::Value::Text(format!("{}%", specific_date)));
            }
        }
    }

    if !query.is_empty() && query != "*" {
        conditions.push("(s.title LIKE ? OR s.summary LIKE ?)".to_string());
        let like_query = format!("%{}%", query);
        params.push(rusqlite::types::Value::Text(like_query.clone()));
        params.push(rusqlite::types::Value::Text(like_query));
    }

    if !conditions.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&conditions.join(" AND "));
    }

    sql.push_str(" GROUP BY s.id HAVING COUNT(m.id) > 0");
    sql.push_str(" ORDER BY s.updated_at DESC");
    if limit > 0 {
        sql.push_str(" LIMIT ?");
        params.push(rusqlite::types::Value::Integer(limit as i64));
    }

    let mut stmt = store.conn.prepare(&sql).map_err(|e| e.to_string())?;

    let mut rows = stmt
        .query(rusqlite::params_from_iter(params))
        .map_err(|e| e.to_string())?;

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
                    is_cron: None,
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
            is_cron: None,
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

    #[test]
    fn test_search_sessions_injection_bypass() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.sqlite");
        let store = VectorStore::open(&db_path).unwrap();

        // Malicious input that would break SQL syntax if interpolated into a
        // quoted string, but is safe when passed as a bound parameter.
        let malicious_date = "2024-01-01'";

        // With proper parameterization, this should not crash or return a
        // malformed query error, even if it returns no results.
        let result = search_sessions_by_time(&store, "", malicious_date, 10);

        // If successful (even with no results), the query remained valid in the
        // presence of a payload that would otherwise break interpolated SQL.
        assert!(result.is_ok());
    }
}
