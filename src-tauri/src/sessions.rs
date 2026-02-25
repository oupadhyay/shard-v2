use crate::agent::ChatMessage;
use crate::config::AppConfig;
use regex::Regex;
use reqwest::Client;
use tauri::{AppHandle, Runtime};

/// System prompt for background LLM to generate a descriptive slug and summary for the session
const SESSION_ANALYSIS_PROMPT: &str = r#"Analyze this conversation and provide two things:
1. SLUG: A 2-4 word descriptive slug using ONLY lowercase letters and hyphens (no spaces/punctuation).
2. SUMMARY: A concise 1-2 sentence summary of the main topics discussed and any conclusions drawn.

Respond strictly in this format:
SLUG: the-generated-slug
SUMMARY: The concise summary text goes here."#;

/// Archives the provided chat history as a markdown session transcript with an LLM-generated slug and summary.
pub async fn archive_session_transcript<R: Runtime>(
    app_handle: &AppHandle<R>,
    http_client: &Client,
    config: &AppConfig,
    session_id: &str,
    history: Vec<ChatMessage>,
) -> Result<(), String> {
    // Only archive if there's meaningful dialogue (e.g. at least 4 messages: 2 user + 2 model turns)
    if history.len() < 4 {
        log::info!("[Sessions] Skipping session archive (history too short)");
        return Ok(());
    }

    // 1. Determine the slug and summary using the background LLM if available
    let (slug, summary) = match &config.gemini_api_key {
        Some(_) | None => {
            let conversation_summary_text: String = history
                .iter()
                .filter(|m| m.role == "user" || m.role == "assistant" || m.role == "model")
                .filter_map(|m| m.content.as_ref().map(|c| format!("{}: {}", m.role, c)))
                .collect::<Vec<_>>()
                .join("\n");

            // Limit to ~25,000 tokens (estimated 100k characters) to avoid excessive background processing
            let mut summary_text = conversation_summary_text;
            if summary_text.len() > 100_000 {
                summary_text.truncate(100_000);
                summary_text.push_str("... [Truncated for analysis]");
            }

            let prompt = format!(
                "{}\n\n---\nConversation Excerpt:\n{}",
                SESSION_ANALYSIS_PROMPT, summary_text
            );

            let model = config
                .background_model
                .as_deref()
                .unwrap_or("gpt-oss-120b (Groq)");

            match crate::background::call_background_llm(http_client, config, model, &prompt).await
            {
                Ok(response) => parse_llm_response(&response),
                Err(e) => {
                    log::warn!(
                        "[Sessions] Session analysis LLM failed, using fallback: {}",
                        e
                    );
                    ("session".to_string(), "No summary generated.".to_string())
                }
            }
        }
    };

    // 2. Format title and update DB
    let safe_slug = if slug.is_empty() {
        "Session".to_string()
    } else {
        slug.split('-')
            .map(|word| {
                let mut c = word.chars();
                match c.next() {
                    None => String::new(),
                    Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                }
            })
            .collect::<Vec<String>>()
            .join(" ")
    };

    if let Ok(store) = crate::memories::get_vector_store(app_handle) {
        let now = chrono::Utc::now().to_rfc3339();

        // Try to preserve original created_at and active_skills if modifying an existing session
        let existing = store.conn.query_row(
            "SELECT created_at, active_skills FROM sessions WHERE id = ?1",
            rusqlite::params![session_id],
            |row| Ok((row.get::<_, String>(0).ok(), row.get::<_, String>(1).ok())),
        ).or_else(|_| {
            store.conn.query_row(
                "SELECT created_at FROM sessions WHERE id = ?1",
                rusqlite::params![session_id],
                |row| Ok((row.get::<_, String>(0).ok(), None)),
            )
        }).unwrap_or((None, None));

        let session_row = crate::db::sessions::SessionRow {
            id: session_id.to_string(),
            title: safe_slug.clone(),
            summary: Some(summary.clone()),
            created_at: existing.0.unwrap_or_else(|| now.clone()),
            updated_at: now,
            active_skills: existing.1.or(Some("[]".to_string())),
        };

        if let Err(e) = crate::db::sessions::insert_session(&store, &session_row) {
            log::warn!("[Sessions] Failed to update session metadata: {}", e);
        } else {
            log::info!("[Sessions] Saved session metadata to SQLite DB (Title: {}, Summary: {})", safe_slug, summary);
        }
    }

    Ok(())
}

/// Parses the LLM format (SLUG: ... \n SUMMARY: ...) into a tuple
pub fn parse_llm_response(response: &str) -> (String, String) {
    let mut slug = String::new();
    let mut summary = String::new();
    let mut in_summary_section = false;

    for line in response.lines() {
        if line.starts_with("SLUG:") {
            slug = sanitize_slug(line.trim_start_matches("SLUG:"));
            in_summary_section = false;
        } else if line.starts_with("SUMMARY:") {
            summary = line.trim_start_matches("SUMMARY:").trim().to_string();
            in_summary_section = true;
        } else if in_summary_section && !line.trim().is_empty() {
            // Append multi-line summary if necessary
            if !summary.is_empty() {
                summary.push(' ');
            }
            summary.push_str(line.trim());
        }
    }

    if slug.is_empty() {
        slug = "session".to_string();
    }
    if summary.is_empty() {
        summary = "No summary generated.".to_string();
    }

    (slug, summary)
}

/// Formats the standard ChatMessage history into a readable Markdown document
pub fn format_transcript(history: &[ChatMessage]) -> String {
    let mut out = String::new();

    for msg in history {
        // Humanize the roles
        let display_role = match msg.role.as_str() {
            "user" => "### User",
            "model" | "assistant" => "### Assistant",
            "system" => "### System",
            _ => "### System/Tool",
        };

        let mut added_content = false;

        if let Some(content) = &msg.content {
            if !content.trim().is_empty() {
                out.push_str(&format!("{}\n\n{}\n\n", display_role, content.trim()));
                added_content = true;
            }
        }

        if let Some(reasoning) = &msg.reasoning {
            if !reasoning.trim().is_empty() {
                if !added_content {
                    out.push_str(&format!("{}\n\n", display_role));
                }
                out.push_str(&format!(
                    "*Thought process:*\n\n> {}\n\n",
                    reasoning.trim().replace('\n', "\n> ")
                ));
                added_content = true;
            }
        }

        if let Some(tool_calls) = &msg.tool_calls {
            if !tool_calls.is_empty() {
                if !added_content {
                    out.push_str(&format!("{}\n\n", display_role));
                }
                for tc in tool_calls {
                    out.push_str(&format!("*Tool Call:* `{}`\n\n", tc.function.name));
                }
            }
        }
    }

    out
}

/// Sanitizes the LLM response to ensure it's a valid, lowercase, hyphenated slug
pub fn sanitize_slug(input: &str) -> String {
    let lowercase = input.trim().to_lowercase();
    // Replace any non-alphanumeric char with a hyphen
    let re = Regex::new(r"[^a-z0-9]+").unwrap();
    let replaced = re.replace_all(&lowercase, "-");
    // Trim hyphens from start and end
    replaced.trim_matches('-').to_string()
}
