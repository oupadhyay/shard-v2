use crate::agent::ChatMessage;
use crate::config::AppConfig;
use chrono::Utc;
use regex::Regex;
use reqwest::Client;
use std::fs::OpenOptions;
use std::io::Write;
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

    // 2. Format the transcript
    let transcript = format_transcript(&history);

    // 3. Write to the memory/sessions directory
    let memory_dir = crate::memories::get_memory_transcripts_dir(app_handle)?;

    let today = Utc::now().format("%Y-%m-%d").to_string();
    let safe_slug = if slug.is_empty() {
        "session".to_string()
    } else {
        slug
    };

    // Before writing a new file, check if an older file for this same session ID exists and delete it
    // This allows the slug to change while keeping the file cleanly overwritten
    let session_suffix = format!("-{}.md", session_id);
    if let Ok(entries) = std::fs::read_dir(&memory_dir) {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                if name.ends_with(&session_suffix) {
                    log::info!("[Sessions] Deleting old transcript for session {}: {}", session_id, name);
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
    }

    let filename = format!("{}-{}{}", today, safe_slug, session_suffix);
    let path = memory_dir.join(&filename);

    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&path)
        .map_err(|e| format!("Failed to create session file: {}", e))?;

    let header = format!(
        "# Session Transcript\n\n*Saved on: {}*\n\n## Summary\n\n{}\n\n---\n\n## Dialogue\n\n",
        Utc::now().to_rfc2822(),
        summary
    );
    file.write_all(header.as_bytes())
        .map_err(|e| format!("Failed to write to session file: {}", e))?;

    file.write_all(transcript.as_bytes())
        .map_err(|e| format!("Failed to write transcript: {}", e))?;

    log::info!("[Sessions] Saved session transcript to {}", path.display());
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
