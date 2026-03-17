/**
 * Heartbeat Engine Module
 *
 * Replaces the old cron_jobs system with file-based heartbeat specs.
 * Each heartbeat is a `.md` file with YAML frontmatter defining schedule,
 * session namespace, optional persona, and rate limits.
 *
 * Key design:
 * - Each heartbeat runs in its own persistent SQLite session (never touches
 *   the user's active Agent singleton).
 * - Uses `call_llm_oneshot` (non-streaming) for background LLM calls.
 * - Draft-before-act: high-risk tool calls are serialized as drafts in the
 *   `proactive_queue` table for user approval.
 */
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex as StdMutex;
use tauri::{AppHandle, Emitter, Manager, Runtime};

// ============================================================================
// Heartbeat Spec Types
// ============================================================================

/// A parsed heartbeat specification from a `.md` file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatSpec {
    /// Cron expression (e.g., "0 */2 * * *")
    pub schedule: String,
    /// Stable session namespace (e.g., "agent:news"). Reused across runs.
    pub session: String,
    /// Optional persona to load for this heartbeat.
    #[serde(default)]
    pub persona: Option<String>,
    /// Maximum tool calls per run (default: 5).
    #[serde(default = "default_max_tool_calls")]
    pub max_tool_calls: u32,
    /// Optional daily run cap (default: 10).
    #[serde(default = "default_max_runs_per_day")]
    pub max_runs_per_day: Option<u32>,
    /// The prompt template body.
    pub prompt: String,
    /// Source filename (for logging).
    #[serde(skip)]
    pub filename: String,
}

fn default_max_tool_calls() -> u32 {
    5
}

fn default_max_runs_per_day() -> Option<u32> {
    Some(10)
}

/// Status info for a heartbeat (returned to frontend dashboard).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatStatusInfo {
    pub filename: String,
    pub schedule: String,
    pub session: String,
    pub persona: Option<String>,
    pub max_tool_calls: u32,
    pub max_runs_per_day: Option<u32>,
    pub prompt_preview: String,
}

/// Returns status info for all loaded heartbeat specs.
pub fn get_heartbeat_status_list<R: Runtime>(app_handle: &AppHandle<R>) -> Vec<HeartbeatStatusInfo> {
    load_heartbeat_specs(app_handle)
        .into_iter()
        .map(|spec| {
            let preview = if spec.prompt.len() > 80 {
                let boundary = spec.prompt.floor_char_boundary(80);
                format!("{}...", &spec.prompt[..boundary])
            } else {
                spec.prompt.clone()
            };

            // Format next occurrence for UI preview
            let schedule_preview = if let Ok(schedule) = spec.schedule.parse::<cron::Schedule>() {
                if let Some(next) = schedule.upcoming(chrono::Local).next() {
                    // E.g., "Tomorrow at 11:05 PM" or "Mar 17 at 11:05 PM"
                    let now = chrono::Local::now();
                    let date_format = if next.date_naive() == now.date_naive() {
                        "Today at %I:%M %p"
                    } else if next.date_naive() == now.date_naive() + chrono::Duration::days(1) {
                        "Tomorrow at %I:%M %p"
                    } else {
                        "%b %d at %I:%M %p"
                    };
                    format!("{}", next.format(date_format))
                } else {
                    spec.schedule.clone()
                }
            } else {
                format!("Invalid cron: {}", spec.schedule)
            };

            HeartbeatStatusInfo {
                filename: spec.filename,
                schedule: schedule_preview,
                session: spec.session,
                persona: spec.persona,
                max_tool_calls: spec.max_tool_calls,
                max_runs_per_day: spec.max_runs_per_day,
                prompt_preview: preview,
            }
        })
        .collect()
}

// ============================================================================
// Heartbeat Spec Parsing
// ============================================================================

/// Parses a heartbeat `.toml` file into a `HeartbeatSpec`.
///
/// Expected format:
/// ```toml
/// schedule = "0 */2 * * *"
/// session = "agent:news"
/// persona = "news-analyst"          # optional
/// max_tool_calls = 3                # optional, default 5
/// max_runs_per_day = 6              # optional
/// prompt = "The prompt body goes here."
/// ```
pub fn parse_heartbeat_spec(content: &str, filename: &str) -> Result<HeartbeatSpec, String> {
    let mut spec: HeartbeatSpec = toml::from_str(content).map_err(|e| {
        format!("Error parsing TOML in heartbeat spec '{}': {}", filename, e)
    })?;

    if spec.prompt.trim().is_empty() {
        return Err(format!(
            "Heartbeat spec '{}' has an empty prompt",
            filename
        ));
    }

    spec.filename = filename.to_string();

    Ok(spec)
}
// ============================================================================
// Heartbeat Discovery
// ============================================================================

/// Returns the directory path for heartbeat specs.
pub fn get_heartbeats_dir<R: Runtime>(app_handle: &AppHandle<R>) -> Result<PathBuf, String> {
    let app_data_dir = app_handle
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to get app data dir: {}", e))?;
    let dir = app_data_dir.join("heartbeats");
    if !dir.exists() {
        fs::create_dir_all(&dir)
            .map_err(|e| format!("Failed to create heartbeats directory: {}", e))?;
    }
    Ok(dir)
}

/// Scans the heartbeats directory and returns all valid specs.
pub fn load_heartbeat_specs<R: Runtime>(app_handle: &AppHandle<R>) -> Vec<HeartbeatSpec> {
    let dir = match get_heartbeats_dir(app_handle) {
        Ok(d) => d,
        Err(e) => {
            log::warn!("[Heartbeat] Failed to get heartbeats dir: {}", e);
            return Vec::new();
        }
    };

    let mut specs = Vec::new();
    let entries = match fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("toml") {
            let filename = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string();

            match fs::read_to_string(&path) {
                Ok(content) => match parse_heartbeat_spec(&content, &filename) {
                    Ok(spec) => {
                        log::info!(
                            "[Heartbeat] Loaded spec '{}' (schedule: {}, session: {})",
                            filename,
                            spec.schedule,
                            spec.session
                        );
                        specs.push(spec);
                    }
                    Err(e) => log::warn!("[Heartbeat] Skipping invalid spec '{}': {}", filename, e),
                },
                Err(e) => log::warn!("[Heartbeat] Failed to read '{}': {}", filename, e),
            }
        }
    }

    specs
}

// ============================================================================
// Rate Limiter
// ============================================================================

/// Global heartbeat rate limiter.
/// Enforces: sequential execution, global cooldown, per-spec daily caps, backoff on 429.
pub struct HeartbeatRateLimiter {
    /// Minimum gap (seconds) between any two heartbeat runs.
    pub global_cooldown_secs: u64,
    /// Timestamp of the last completed heartbeat run.
    last_run_epoch: AtomicU64,
    /// Per-session daily run counters. Key = session namespace, Value = (date_str, count).
    daily_counters: StdMutex<HashMap<String, (String, u32)>>,
    /// Per-session backoff state. Key = session, Value = next-allowed epoch seconds.
    backoff_until: StdMutex<HashMap<String, u64>>,
}

impl HeartbeatRateLimiter {
    pub fn new(global_cooldown_secs: u64) -> Self {
        Self {
            global_cooldown_secs,
            last_run_epoch: AtomicU64::new(0),
            daily_counters: StdMutex::new(HashMap::new()),
            backoff_until: StdMutex::new(HashMap::new()),
        }
    }

    /// Returns true if the heartbeat should be skipped (rate limited).
    pub fn should_skip(&self, spec: &HeartbeatSpec) -> bool {
        let now = Utc::now().timestamp() as u64;

        // 1. Global cooldown
        let last = self.last_run_epoch.load(Ordering::Relaxed);
        if last > 0 && now.saturating_sub(last) < self.global_cooldown_secs {
            log::info!(
                "[Heartbeat] Skipping '{}': global cooldown ({} secs remaining)",
                spec.session,
                self.global_cooldown_secs - now.saturating_sub(last)
            );
            return true;
        }

        // 2. Backoff check (429/quota error)
        if let Ok(backoffs) = self.backoff_until.lock() {
            if let Some(&until) = backoffs.get(&spec.session) {
                if now < until {
                    log::info!(
                        "[Heartbeat] Skipping '{}': in backoff until {} ({} secs)",
                        spec.session,
                        until,
                        until - now
                    );
                    return true;
                }
            }
        }

        // 3. Daily cap
        if let Some(cap) = spec.max_runs_per_day {
            let today = Utc::now().format("%Y-%m-%d").to_string();
            if let Ok(counters) = self.daily_counters.lock() {
                if let Some((date, count)) = counters.get(&spec.session) {
                    if date == &today && *count >= cap {
                        log::info!(
                            "[Heartbeat] Skipping '{}': daily cap reached ({}/{})",
                            spec.session,
                            count,
                            cap
                        );
                        return true;
                    }
                }
            }
        }

        false
    }

    /// Record a successful heartbeat run.
    pub fn record_run(&self, session: &str) {
        let now = Utc::now().timestamp() as u64;
        self.last_run_epoch.store(now, Ordering::Relaxed);

        let today = Utc::now().format("%Y-%m-%d").to_string();
        if let Ok(mut counters) = self.daily_counters.lock() {
            let entry = counters
                .entry(session.to_string())
                .or_insert_with(|| (today.clone(), 0));
            if entry.0 != today {
                // New day, reset counter
                *entry = (today, 1);
            } else {
                entry.1 += 1;
            }
        }

        // Clear any existing backoff on success
        if let Ok(mut backoffs) = self.backoff_until.lock() {
            backoffs.remove(session);
        }
    }

    /// Record a quota error (429). Applies exponential backoff with jitter.
    pub fn record_quota_error(&self, session: &str) {
        let now = Utc::now().timestamp() as u64;

        if let Ok(mut backoffs) = self.backoff_until.lock() {
            let current_backoff = backoffs.get(session).copied().unwrap_or(0);
            // Exponential: 2m → 4m → 8m → 16m (capped)
            let elapsed_from_backoff = if current_backoff > now {
                current_backoff - now
            } else {
                0
            };
            let next_backoff_secs = if elapsed_from_backoff == 0 {
                120 // Initial: 2 minutes
            } else {
                (elapsed_from_backoff * 2).min(960) // Max: 16 minutes
            };
            // Add jitter (0-30s)
            let jitter = (now % 30) as u64;
            let until = now + next_backoff_secs + jitter;
            backoffs.insert(session.to_string(), until);
            log::warn!(
                "[Heartbeat] Quota error for '{}': backing off for {} secs",
                session,
                next_backoff_secs + jitter
            );
        }
    }
}

// ============================================================================
// Proactive Queue
// ============================================================================

/// A proactive message or draft pending user review.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProactiveMessage {
    pub id: String,
    pub heartbeat_session: String,
    pub content: String,
    /// JSON payload of the pending tool call (None for simple notifications).
    pub draft_payload: Option<String>,
    pub needs_approval: bool,
    pub reviewed_at: Option<String>,
    /// None = pending, Some(true) = approved, Some(false) = rejected.
    pub approved: Option<bool>,
    pub created_at: String,
}

/// Ensure the proactive_queue table exists in the database.
pub fn ensure_proactive_queue_table<R: Runtime>(app_handle: &AppHandle<R>) -> Result<(), String> {
    let store = crate::memories::get_vector_store(app_handle)?;
    store
        .conn
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS proactive_queue (
                id TEXT PRIMARY KEY,
                heartbeat_session TEXT NOT NULL,
                content TEXT NOT NULL,
                draft_payload TEXT,
                needs_approval INTEGER DEFAULT 0,
                reviewed_at TEXT,
                approved INTEGER,
                created_at TEXT NOT NULL
            );",
        )
        .map_err(|e| format!("Failed to create proactive_queue table: {}", e))
}

/// Insert a proactive message into the queue.
pub fn insert_proactive_message<R: Runtime>(
    app_handle: &AppHandle<R>,
    msg: &ProactiveMessage,
) -> Result<(), String> {
    let store = crate::memories::get_vector_store(app_handle)?;
    store
        .conn
        .execute(
            "INSERT INTO proactive_queue (id, heartbeat_session, content, draft_payload, needs_approval, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                msg.id,
                msg.heartbeat_session,
                msg.content,
                msg.draft_payload,
                msg.needs_approval as i32,
                msg.created_at,
            ],
        )
        .map_err(|e| format!("Failed to insert proactive message: {}", e))?;
    Ok(())
}

/// Get unreviewed proactive messages (for frontend polling / badge counts).
pub fn get_unreviewed_messages<R: Runtime>(
    app_handle: &AppHandle<R>,
    limit: usize,
) -> Result<Vec<ProactiveMessage>, String> {
    let store = crate::memories::get_vector_store(app_handle)?;
    let mut stmt = store
        .conn
        .prepare(
            "SELECT id, heartbeat_session, content, draft_payload, needs_approval, reviewed_at, approved, created_at
             FROM proactive_queue
             WHERE reviewed_at IS NULL
             ORDER BY created_at DESC
             LIMIT ?1",
        )
        .map_err(|e| format!("Failed to prepare query: {}", e))?;

    let messages = stmt
        .query_map(rusqlite::params![limit as i64], |row| {
            Ok(ProactiveMessage {
                id: row.get(0)?,
                heartbeat_session: row.get(1)?,
                content: row.get(2)?,
                draft_payload: row.get(3)?,
                needs_approval: row.get::<_, i32>(4)? != 0,
                reviewed_at: row.get(5)?,
                approved: row.get::<_, Option<i32>>(6)?.map(|v| v != 0),
                created_at: row.get(7)?,
            })
        })
        .map_err(|e| format!("Failed to query proactive messages: {}", e))?
        .filter_map(|r| r.ok())
        .collect();

    Ok(messages)
}

/// Mark a proactive message as reviewed (with optional approval for drafts).
pub fn review_proactive_message<R: Runtime>(
    app_handle: &AppHandle<R>,
    message_id: &str,
    approved: Option<bool>,
) -> Result<(), String> {
    let store = crate::memories::get_vector_store(app_handle)?;
    let now = Utc::now().to_rfc3339();
    store
        .conn
        .execute(
            "UPDATE proactive_queue SET reviewed_at = ?1, approved = ?2 WHERE id = ?3",
            rusqlite::params![now, approved.map(|a| a as i32), message_id],
        )
        .map_err(|e| format!("Failed to review proactive message: {}", e))?;
    Ok(())
}

/// Get the count of unreviewed proactive messages for a specific heartbeat session.
pub fn get_unreviewed_count<R: Runtime>(
    app_handle: &AppHandle<R>,
    heartbeat_session: Option<&str>,
) -> Result<usize, String> {
    let store = crate::memories::get_vector_store(app_handle)?;
    let count: i64 = if let Some(session) = heartbeat_session {
        store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM proactive_queue WHERE reviewed_at IS NULL AND heartbeat_session = ?1",
                rusqlite::params![session],
                |row| row.get(0),
            )
            .map_err(|e| format!("Failed to count: {}", e))?
    } else {
        store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM proactive_queue WHERE reviewed_at IS NULL",
                [],
                |row| row.get(0),
            )
            .map_err(|e| format!("Failed to count: {}", e))?
    };
    Ok(count as usize)
}

// ============================================================================
// Heartbeat Turn Processing
// ============================================================================

/// Processes a single heartbeat turn. Completely independent of the Agent singleton.
/// Execute a single heartbeat turn with multi-turn tool calling.
///
/// 1. Loads or creates the heartbeat's persistent session in SQLite.
/// 2. Assembles context (session history + persona).
/// 3. Enters a tool-calling loop:
///    a. Calls the background LLM with tool definitions from `get_heartbeat_tools()`.
///    b. Safe tools are executed inline and fed back to the LLM.
///    c. Draft-gated tools (config/heartbeat mutations) are intercepted,
///       serialized as drafts into `proactive_queue`, and the loop halts.
///    d. Loop continues until "stop" or `max_tool_calls` reached.
/// 4. Persists the final messages to the heartbeat's session.
/// 5. Returns the LLM response content (or inserts into proactive_queue).
pub async fn process_heartbeat_turn<R: Runtime>(
    app_handle: &AppHandle<R>,
    spec: &HeartbeatSpec,
) -> Result<Option<String>, String> {
    let config = crate::config::load_config(app_handle)?;
    let background_model = config
        .background_model
        .as_deref()
        .unwrap_or(crate::background::DEFAULT_BACKGROUND_MODEL);

    // Verify API key availability
    let _ = config.get_model_provider_config(background_model, "heartbeat")?;

    let http_client = reqwest::Client::new();
    let session_id = &spec.session;

    // 1. Ensure session exists in DB
    ensure_heartbeat_session(app_handle, session_id)?;

    // 2. Load recent history for context
    let history_context = load_session_history_context(app_handle, session_id, 20)?;

    // 3. Build the system prompt
    let mut system_parts = vec![format!(
        "You are Shard running a background heartbeat task '{}'. Today is {}.",
        spec.session,
        Utc::now().format("%Y-%m-%d")
    )];

    if let Some(persona_name) = &spec.persona {
        if let Some(persona_content) = crate::personas::get_persona_content(persona_name) {
            system_parts.push(format!("\n\nActive Persona:\n{}", persona_content));
        }
    }

    system_parts.push(
        "\n\nYou are running autonomously in the background. Be concise. \
         You have access to tools — use them when needed. \
         If you want to modify Shard's configuration or heartbeats, use the appropriate tool. \
         If nothing actionable is found, respond with just: HEARTBEAT_OK"
            .to_string(),
    );

    let system_prompt = system_parts.join("");

    // 4. Build initial messages array
    let user_content = if history_context.is_empty() {
        spec.prompt.clone()
    } else {
        format!(
            "Previous context from this heartbeat session:\n{}\n\n---\n\nCurrent task:\n{}",
            history_context, spec.prompt
        )
    };

    let mut messages = vec![
        serde_json::json!({ "role": "system", "content": system_prompt }),
        serde_json::json!({ "role": "user", "content": user_content }),
    ];

    // 5. Tool-calling loop
    let active_personas = if let Some(p) = &spec.persona {
        vec![p.clone()]
    } else {
        vec![]
    };
    let tools = crate::tools::get_heartbeat_tools(&active_personas);
    let max_iterations = spec.max_tool_calls as usize;
    let mut final_content: Option<String> = None;
    let mut tool_calls_made = 0usize;

    for iteration in 0..max_iterations {
        log::info!(
            "[Heartbeat] '{}' tool loop iteration {} / {}",
            spec.session,
            iteration + 1,
            max_iterations
        );

        let llm_response = crate::background::call_llm_with_tools(
            &http_client,
            &config,
            background_model,
            &messages,
            &tools,
            2000,
            0.3,
        )
        .await?;

        // If no tool calls, we're done
        if llm_response.tool_calls.is_empty() {
            final_content = llm_response.content;
            break;
        }

        // Add assistant message with tool_calls to conversation
        let tool_calls_json: Vec<serde_json::Value> = llm_response
            .tool_calls
            .iter()
            .map(|tc| {
                serde_json::json!({
                    "id": tc.id,
                    "type": "function",
                    "function": {
                        "name": tc.name,
                        "arguments": tc.arguments
                    }
                })
            })
            .collect();

        let mut assistant_msg = serde_json::json!({
            "role": "assistant",
            "tool_calls": tool_calls_json
        });
        if let Some(ref content) = llm_response.content {
            assistant_msg["content"] = serde_json::Value::String(content.clone());
        }
        messages.push(assistant_msg);

        // Process each tool call
        let mut draft_created = false;
        for tc in &llm_response.tool_calls {
            let args: serde_json::Value =
                serde_json::from_str(&tc.arguments).unwrap_or(serde_json::json!({}));

            if crate::tools::is_draft_gated(&tc.name) {
                // Draft-gated tool: build justification from the tool call arguments,
                // since content is often None when the model returns tool_calls.
                let justification = format!(
                    "`{}` with arguments: {}",
                    tc.name,
                    serde_json::to_string_pretty(&args).unwrap_or_else(|_| tc.arguments.clone())
                );

                match create_draft_for_tool_call(app_handle, spec, &tc.name, &args, &justification) {
                    Ok(msg_id) => {
                        log::info!(
                            "[Heartbeat] '{}': draft created for '{}' (msg_id: {})",
                            spec.session,
                            tc.name,
                            msg_id
                        );
                        // Add tool response indicating draft was queued
                        messages.push(serde_json::json!({
                            "role": "tool",
                            "tool_call_id": tc.id,
                            "content": format!("Action '{}' has been queued for user approval. The user will be notified.", tc.name)
                        }));
                    }
                    Err(e) => {
                        log::error!("[Heartbeat] Failed to create draft: {}", e);
                        messages.push(serde_json::json!({
                            "role": "tool",
                            "tool_call_id": tc.id,
                            "content": format!("Error creating draft: {}", e)
                        }));
                    }
                }
                draft_created = true;
            } else {
                // Safe tool: execute immediately
                let result = execute_safe_tool(app_handle, &http_client, &config, spec, &tc.name, &args).await;
                log::info!(
                    "[Heartbeat] '{}': executed tool '{}' -> {} chars",
                    spec.session,
                    tc.name,
                    result.len()
                );
                messages.push(serde_json::json!({
                    "role": "tool",
                    "tool_call_id": tc.id,
                    "content": result
                }));
            }
            tool_calls_made += 1;
        }

        // If a draft was created, halt the loop — the user needs to approve first
        if draft_created {
            log::info!(
                "[Heartbeat] '{}': halting tool loop — draft awaiting approval",
                spec.session
            );
            // Set final content to indicate the draft was created
            final_content = Some("Draft action queued for user approval.".to_string());
            break;
        }

        // If we've exhausted tool calls, break
        if tool_calls_made >= max_iterations {
            log::info!(
                "[Heartbeat] '{}': max tool calls ({}) reached",
                spec.session,
                max_iterations
            );
            break;
        }
    }

    // 6. Persist the turn to the heartbeat's session
    let response_text = final_content.as_deref().unwrap_or("HEARTBEAT_OK");
    persist_heartbeat_turn(app_handle, session_id, &spec.prompt, response_text)?;

    // 7. Determine output
    let trimmed = response_text.trim();
    if trimmed == "HEARTBEAT_OK" || trimmed.is_empty() || trimmed == "Draft action queued for user approval." {
        log::info!("[Heartbeat] '{}': nothing to report to user", spec.session);
        Ok(None)
    } else {
        log::info!(
            "[Heartbeat] '{}': produced {} chars of output",
            spec.session,
            trimmed.len()
        );

        // Insert as a proactive notification
        let msg = ProactiveMessage {
            id: uuid::Uuid::new_v4().to_string(),
            heartbeat_session: session_id.clone(),
            content: trimmed.to_string(),
            draft_payload: None,
            needs_approval: false,
            reviewed_at: None,
            approved: None,
            created_at: Utc::now().to_rfc3339(),
        };

        if let Err(e) = insert_proactive_message(app_handle, &msg) {
            log::warn!("[Heartbeat] Failed to insert proactive message: {}", e);
        }

        app_handle.emit("proactive-message", &msg).ok();

        Ok(Some(trimmed.to_string()))
    }
}

/// Execute a safe (non-draft-gated) tool during a heartbeat run.
/// Mirrors the Agent's execute_tool_uncached logic for the subset of tools
/// available to heartbeats.
async fn execute_safe_tool<R: Runtime>(
    app_handle: &AppHandle<R>,
    http_client: &reqwest::Client,
    config: &crate::config::AppConfig,
    _spec: &HeartbeatSpec,
    tool_name: &str,
    args: &serde_json::Value,
) -> String {
    use crate::integrations::*;

    match tool_name {
        "web_search" => {
            let query = args["query"].as_str().unwrap_or_default();
            match web_search::perform_web_search(query, config.brave_api_key.as_deref()).await {
                Ok(results) => serde_json::to_string(&results)
                    .unwrap_or_else(|_| "Failed to serialize search results".to_string()),
                Err(e) => format!("Error: {}", e),
            }
        }
        "open_url" => {
            let url = args["url"].as_str().unwrap_or_default();
            match browser::read_url(http_client, url).await {
                Ok(md) => format!("Read URL: {}\n\n{}", url, md),
                Err(e) => format!("Error: {}", e),
            }
        }
        "search_wikipedia" => {
            let query = args["query"].as_str().unwrap_or_default();
            match wikipedia::perform_wikipedia_lookup(http_client, query).await {
                Ok(Some((title, summary, _))) => format!("Wikipedia: {}\n{}", title, summary),
                Ok(None) => "No Wikipedia results found.".to_string(),
                Err(e) => format!("Error: {}", e),
            }
        }
        "search_arxiv" => {
            let query = args["query"].as_str().unwrap_or_default();
            match arxiv::perform_arxiv_lookup(http_client, query, 3).await {
                Ok(papers) => {
                    let summaries: Vec<String> = papers
                        .iter()
                        .map(|p| format!("- [{}] {}: {}", p.id, p.title, p.summary))
                        .collect();
                    format!("ArXiv Results:\n{}", summaries.join("\n\n"))
                }
                Err(e) => format!("Error: {}", e),
            }
        }
        "read_arxiv_paper" => {
            let paper_id = args["paper_id"].as_str().unwrap_or_default();
            match arxiv::read_arxiv_paper(http_client, paper_id).await {
                Ok(paper) => format!("# {}\n\n**Abstract:** {}\n\n{}", paper.title, paper.abstract_text, paper.content),
                Err(e) => format!("Error: {}", e),
            }
        }
        "save_memory" => {
            if config.incognito_mode.unwrap_or(false) {
                return "Skipped: Memory saving is disabled in incognito mode.".to_string();
            }
            let category_str = args["category"].as_str().unwrap_or("fact");
            let content = args["content"].as_str().unwrap_or_default().to_string();
            let importance = args["importance"].as_u64().unwrap_or(3) as u8;

            let category = match category_str {
                "preference" => crate::memories::MemoryCategory::Preference,
                "project" => crate::memories::MemoryCategory::Project,
                "interaction" => crate::memories::MemoryCategory::Interaction,
                _ => crate::memories::MemoryCategory::Fact,
            };

            match crate::memories::add_memory(app_handle, category, content.clone(), importance) {
                Ok(_) => format!("Memory saved: {}", content),
                Err(e) => format!("Error saving memory: {}", e),
            }
        }
        "update_topic_summary" => {
            if config.incognito_mode.unwrap_or(false) {
                return "Skipped: Incognito mode.".to_string();
            }
            let topic = args["topic"].as_str().unwrap_or_default();
            let content = args["content"].as_str().unwrap_or_default();
            match crate::memories::update_topic_summary(app_handle, topic, content) {
                Ok(_) => format!("Topic '{}' updated.", topic),
                Err(e) => format!("Error: {}", e),
            }
        }
        "read_topic_summary" => {
            let topic = args["topic"].as_str().unwrap_or_default();
            match crate::memories::read_topic_summary(app_handle, topic) {
                Ok(content) => content,
                Err(e) => format!("Error: {}", e),
            }
        }
        "memory_search" => {
            // Hybrid search: compute embedding via Gemini API, then BM25 + dense fusion
            let query = args["query"].as_str().unwrap_or_default();
            let gemini_key = match config.gemini_api_key.as_deref() {
                Some(k) if !k.is_empty() => k,
                _ => return "Error: Gemini API key required for memory search.".to_string(),
            };
            let query_embedding = match crate::interactions::generate_embedding(http_client, query, gemini_key).await {
                Ok(emb) => emb,
                Err(e) => return format!("Error computing embedding: {}", e),
            };
            match crate::interactions::hybrid_search_interactions(app_handle, query, &query_embedding, 5) {
                Ok(results) => {
                    if results.is_empty() {
                        "No relevant memories found.".to_string()
                    } else {
                        results
                            .iter()
                            .map(|r| format!("[{}] {}: {}", r.ts.format("%Y-%m-%d"), r.role, r.content))
                            .collect::<Vec<_>>()
                            .join("\n---\n")
                    }
                }
                Err(e) => format!("Error: {}", e),
            }
        }
        "wake_me_up_in" => {
            // wake_me_up_in is not supported in the heartbeat safe tool executor
            // because heartbeats already have cron scheduling.
            "Tool 'wake_me_up_in' is not available. Use the heartbeat's cron schedule for recurring tasks.".to_string()
        }
        "get_weather" => {
            let location = args["location"].as_str().unwrap_or_default();
            match crate::integrations::weather::perform_weather_lookup(http_client, location).await {
                Ok(json_str) => json_str,
                Err(e) => format!("Error: {}", e),
            }
        }
        "get_stock_price" => {
            let symbol = args["symbol"].as_str().unwrap_or_default();
            match crate::integrations::finance::perform_finance_lookup(symbol).await {
                Ok(result) => result,
                Err(e) => format!("Error: {}", e),
            }
        }
        "run_python" => {
            "Tool 'run_python' is currently not available in heartbeat background runs.".to_string()
        }
        "youtube_transcript" => {
            "Tool 'youtube_transcript' is too large for heartbeat background runs. Please summarize manually or run in chat.".to_string()
        }
        _ => format!("Unknown tool: {}", tool_name),
    }
}

/// Ensure a session row exists for the heartbeat in SQLite.
fn ensure_heartbeat_session<R: Runtime>(
    app_handle: &AppHandle<R>,
    session_id: &str,
) -> Result<(), String> {
    let store = crate::memories::get_vector_store(app_handle)?;
    let exists: bool = store
        .conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sessions WHERE id = ?1",
            rusqlite::params![session_id],
            |row| row.get(0),
        )
        .unwrap_or(false);

    if !exists {
        let now = Utc::now().to_rfc3339();
        let session = crate::db::sessions::SessionRow {
            id: session_id.to_string(),
            title: format!("Heartbeat: {}", session_id),
            summary: Some("Autonomous heartbeat session".to_string()),
            created_at: now.clone(),
            updated_at: now,
            active_personas: Some("[]".to_string()),
        };
        crate::db::sessions::insert_session(&store, &session)?;
        log::info!("[Heartbeat] Created new session for '{}'", session_id);
    }

    Ok(())
}

/// Load recent conversation history from a heartbeat session for context.
fn load_session_history_context<R: Runtime>(
    app_handle: &AppHandle<R>,
    session_id: &str,
    max_messages: usize,
) -> Result<String, String> {
    let store = crate::memories::get_vector_store(app_handle)?;

    let mut stmt = store
        .conn
        .prepare(
            "SELECT role, content FROM messages WHERE session_id = ?1 ORDER BY created_at DESC LIMIT ?2",
        )
        .map_err(|e| format!("Failed to prepare history query: {}", e))?;

    let rows: Vec<(String, String)> = stmt
        .query_map(rusqlite::params![session_id, max_messages as i64], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|e| format!("Failed to query history: {}", e))?
        .filter_map(|r| r.ok())
        .collect();

    if rows.is_empty() {
        return Ok(String::new());
    }

    // Reverse to chronological order
    let mut context = String::new();
    for (role, content) in rows.into_iter().rev() {
        // Try to parse as ChatMessage JSON, fall back to raw content
        let display_content = if let Ok(msg) =
            serde_json::from_str::<crate::agent::ChatMessage>(&content)
        {
            msg.content.unwrap_or_default()
        } else {
            content
        };

        if !display_content.is_empty() {
            // Truncate long messages
            let truncated = if display_content.len() > 300 {
                let boundary = display_content.floor_char_boundary(300);
                format!("{}...", &display_content[..boundary])
            } else {
                display_content
            };
            context.push_str(&format!("{}: {}\n", role, truncated));
        }
    }

    Ok(context)
}

/// Persist a heartbeat turn (user prompt + assistant response) to the session.
fn persist_heartbeat_turn<R: Runtime>(
    app_handle: &AppHandle<R>,
    session_id: &str,
    prompt: &str,
    response: &str,
) -> Result<(), String> {
    let store = crate::memories::get_vector_store(app_handle)?;
    let now = Utc::now().to_rfc3339();

    // Insert the "user" (heartbeat prompt) message
    let user_msg = crate::agent::ChatMessage {
        role: "user".to_string(),
        content: Some(prompt.to_string()),
        reasoning: None,
        tool_calls: None,
        tool_call_id: None,
        is_cron: Some(true), // Mark as background-originated
        images: None,
    };
    let user_row = crate::db::sessions::MessageRow {
        id: uuid::Uuid::new_v4().to_string(),
        session_id: session_id.to_string(),
        role: "user".to_string(),
        content: serde_json::to_string(&user_msg).unwrap_or_else(|_| "{}".to_string()),
        created_at: now.clone(),
    };
    crate::db::sessions::insert_message(&store, &user_row)?;

    // Insert the assistant response
    let assistant_msg = crate::agent::ChatMessage {
        role: "assistant".to_string(),
        content: Some(response.to_string()),
        reasoning: None,
        tool_calls: None,
        tool_call_id: None,
        is_cron: Some(true),
        images: None,
    };
    let assistant_row = crate::db::sessions::MessageRow {
        id: uuid::Uuid::new_v4().to_string(),
        session_id: session_id.to_string(),
        role: "assistant".to_string(),
        content: serde_json::to_string(&assistant_msg).unwrap_or_else(|_| "{}".to_string()),
        created_at: now.clone(),
    };
    crate::db::sessions::insert_message(&store, &assistant_row)?;

    // Update session timestamp
    store
        .conn
        .execute(
            "UPDATE sessions SET updated_at = ?1 WHERE id = ?2",
            rusqlite::params![now, session_id],
        )
        .map_err(|e| format!("Failed to update session timestamp: {}", e))?;

    Ok(())
}

// ============================================================================
// Heartbeat Engine (Scheduler)
// ============================================================================

/// Start the heartbeat engine. Replaces the old cron_jobs scheduler.
/// Scans heartbeat specs, registers each with tokio-cron-scheduler,
/// and manages rate limiting across all heartbeats.
pub fn start_heartbeat_engine<R: Runtime>(app_handle: AppHandle<R>) {
    tauri::async_runtime::spawn(async move {
        // Ensure the proactive_queue table exists
        if let Err(e) = ensure_proactive_queue_table(&app_handle) {
            log::error!("[Heartbeat] Failed to create proactive_queue table: {}", e);
            return;
        }

        let specs = load_heartbeat_specs(&app_handle);
        if specs.is_empty() {
            log::info!("[Heartbeat] No heartbeat specs found. Engine idle.");
            return;
        }

        log::info!(
            "[Heartbeat] Starting engine with {} heartbeat spec(s)",
            specs.len()
        );

        // Load global cooldown from config
        let global_cooldown = crate::config::load_config(&app_handle)
            .ok()
            .and_then(|c| c.heartbeat_global_cooldown_secs)
            .unwrap_or(60);

        let rate_limiter = std::sync::Arc::new(HeartbeatRateLimiter::new(global_cooldown));

        match tokio_cron_scheduler::JobScheduler::new().await {
            Ok(sched) => {
                for spec in specs {
                    let app_h = app_handle.clone();
                    let limiter = rate_limiter.clone();
                    let schedule = spec.schedule.clone();
                    let spec_clone = spec.clone();

                    let job = tokio_cron_scheduler::Job::new_async_tz(
                        schedule.as_str(),
                        chrono::Local,
                        move |_uuid, mut _l| {
                            let app_h2 = app_h.clone();
                            let limiter2 = limiter.clone();
                            let spec2 = spec_clone.clone();
                            Box::pin(async move {
                                log::info!(
                                    "[Heartbeat] Tick for '{}' (spec: {})",
                                    spec2.session,
                                    spec2.filename
                                );

                                // Rate limit check
                                if limiter2.should_skip(&spec2) {
                                    return;
                                }

                                match process_heartbeat_turn(&app_h2, &spec2).await {
                                    Ok(_) => {
                                        limiter2.record_run(&spec2.session);
                                        log::info!(
                                            "[Heartbeat] '{}' completed successfully",
                                            spec2.session
                                        );
                                    }
                                    Err(e) => {
                                        if e.contains("429") || e.contains("quota") || e.contains("rate") {
                                            limiter2.record_quota_error(&spec2.session);
                                        }
                                        log::error!(
                                            "[Heartbeat] '{}' failed: {}",
                                            spec2.session,
                                            e
                                        );
                                    }
                                }
                            })
                        },
                    );

                    match job {
                        Ok(j) => {
                            if let Err(e) = sched.add(j).await {
                                log::error!(
                                    "[Heartbeat] Failed to schedule '{}': {}",
                                    spec.session,
                                    e
                                );
                            } else {
                                log::info!(
                                    "[Heartbeat] Scheduled '{}' with cron: '{}'",
                                    spec.session,
                                    spec.schedule
                                );
                            }
                        }
                        Err(e) => log::error!(
                            "[Heartbeat] Invalid cron '{}' for '{}': {}",
                            spec.schedule,
                            spec.session,
                            e
                        ),
                    }
                }

                if let Err(e) = sched.start().await {
                    log::error!("[Heartbeat] Failed to start scheduler: {}", e);
                } else {
                    log::info!("[Heartbeat] Scheduler started successfully");
                    // Keep the task alive
                    std::future::pending::<()>().await;
                }
            }
            Err(e) => log::error!("[Heartbeat] Failed to create JobScheduler: {}", e),
        }
    });
}

// ============================================================================
// Cron Job Migration
// ============================================================================

/// Migrate legacy `cron_jobs` from config.toml to heartbeat spec files.
/// Called once during `load_config()` when `cron_jobs` field is present.
pub fn migrate_cron_jobs_to_heartbeats<R: Runtime>(
    app_handle: &AppHandle<R>,
    cron_jobs: &[crate::config::LegacyCronJob],
) -> Result<usize, String> {
    if cron_jobs.is_empty() {
        return Ok(0);
    }

    let dir = get_heartbeats_dir(app_handle)?;
    let mut migrated = 0;

    for (i, job) in cron_jobs.iter().enumerate() {
        let filename = format!("migrated-cron-{}.md", i + 1);
        let filepath = dir.join(&filename);

        // Don't overwrite if already migrated
        if filepath.exists() {
            log::info!(
                "[Heartbeat] Migration: '{}' already exists, skipping",
                filename
            );
            continue;
        }

        let content = format!(
            "---\nschedule: \"{}\"\nsession: \"agent:cron-migrated-{}\"\n---\n{}",
            job.schedule,
            i + 1,
            job.prompt
        );

        fs::write(&filepath, &content)
            .map_err(|e| format!("Failed to write migrated heartbeat '{}': {}", filename, e))?;

        log::info!(
            "[Heartbeat] Migrated cron job #{} to '{}'",
            i + 1,
            filename
        );
        migrated += 1;
    }

    Ok(migrated)
}

// ============================================================================
// Draft-Before-Act Orchestrator
// ============================================================================

/// Serialized representation of a draft tool call awaiting user approval.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DraftPayload {
    /// Tool function name (e.g., "create_heartbeat")
    pub name: String,
    /// Serialized tool arguments
    pub arguments: serde_json::Value,
    /// LLM's justification for the action
    pub justification: String,
    /// Which heartbeat session originated this draft
    pub heartbeat_session: String,
}

/// Execute a draft-gated tool after user approval.
/// Called from the `approve_draft` Tauri command.
pub async fn execute_approved_draft<R: Runtime>(
    app_handle: &AppHandle<R>,
    message_id: &str,
) -> Result<String, String> {
    // 1. Fetch the proactive message to get the draft payload
    let store = crate::memories::get_vector_store(app_handle)?;
    let row: (String, String) = store
        .conn
        .query_row(
            "SELECT draft_payload, heartbeat_session FROM proactive_queue WHERE id = ?1",
            rusqlite::params![message_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .map_err(|e| format!("Draft not found: {}", e))?;

    let (draft_json, _session) = row;
    let draft: DraftPayload =
        serde_json::from_str(&draft_json).map_err(|e| format!("Invalid draft payload: {}", e))?;

    log::info!(
        "[DraftAct] Executing approved draft: {} with args: {}",
        draft.name,
        draft.arguments
    );

    // 2. Execute the tool
    let result = execute_draft_gated_tool(app_handle, &draft.name, &draft.arguments).await?;

    // 3. Persist the execution & result to the session's chat history
    let summary = format!("**Executed Draft Action:** `{}`\n```json\n{}\n```\n**Result:**\n{}",
        draft.name,
        serde_json::to_string_pretty(&draft.arguments).unwrap_or_default(),
        result
    );
    let now = chrono::Utc::now().to_rfc3339();
    let msg = crate::db::sessions::MessageRow {
        id: uuid::Uuid::new_v4().to_string(),
        session_id: draft.heartbeat_session.clone(),
        role: "assistant".to_string(),
        content: summary,
        created_at: now,
    };
    if let Err(e) = crate::db::sessions::insert_message(&store, &msg) {
        log::warn!("[DraftAct] Failed to save execution result to history: {}", e);
    }

    // 4. Mark as reviewed + approved
    review_proactive_message(app_handle, message_id, Some(true))?;

    Ok(result)
}

/// Execute a draft-gated tool (config/heartbeat mutations).
/// These only run after explicit user approval.
async fn execute_draft_gated_tool<R: Runtime>(
    app_handle: &AppHandle<R>,
    tool_name: &str,
    args: &serde_json::Value,
) -> Result<String, String> {
    match tool_name {
        "edit_config" => {
            let key = args["key"]
                .as_str()
                .ok_or("Missing 'key' argument")?;
            let value = args["value"]
                .as_str()
                .ok_or("Missing 'value' argument")?;

            let mut config = crate::config::load_config(app_handle)?;

            // Apply the edit based on the key
            match key {
                "selected_model" => config.selected_model = Some(value.to_string()),
                "background_model" => config.background_model = Some(value.to_string()),
                "enable_tools" => config.enable_tools = Some(value.parse::<bool>().unwrap_or(true)),
                "research_mode" => config.research_mode = Some(value.parse::<bool>().unwrap_or(false)),
                "incognito_mode" => config.incognito_mode = Some(value.parse::<bool>().unwrap_or(false)),
                "enable_screen_context" => config.enable_screen_context = Some(value.parse::<bool>().unwrap_or(false)),
                "enable_compaction" => config.enable_compaction = Some(value.parse::<bool>().unwrap_or(true)),
                _ => return Err(format!("Unknown config key: '{}'", key)),
            }

            crate::config::save_config(app_handle, &config)?;
            Ok(format!("Config '{}' updated to '{}'", key, value))
        }
        "create_heartbeat" => {
            let name = args["name"]
                .as_str()
                .ok_or("Missing 'name' argument")?;
            let schedule = args["schedule"]
                .as_str()
                .ok_or("Missing 'schedule' argument")?;
            let session = args["session"]
                .as_str()
                .ok_or("Missing 'session' argument")?;
            let prompt = args["prompt"]
                .as_str()
                .ok_or("Missing 'prompt' argument")?;
            let persona = args["persona"].as_str();
            let max_tool_calls = args["max_tool_calls"].as_u64();

            // Sanitize filename
            let safe_name: String = name
                .chars()
                .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
                .take(64)
                .collect();
            if safe_name.is_empty() {
                return Err("Invalid heartbeat name".to_string());
            }

            let dir = get_heartbeats_dir(app_handle)?;
            let filepath = dir.join(format!("{}.md", safe_name));
            if filepath.exists() {
                return Err(format!("Heartbeat '{}' already exists", safe_name));
            }

            let mut frontmatter = format!(
                "---\nschedule: \"{}\"\nsession: \"{}\"\n",
                schedule, session
            );
            if let Some(p) = persona {
                frontmatter.push_str(&format!("persona: \"{}\"\n", p));
            }
            if let Some(m) = max_tool_calls {
                frontmatter.push_str(&format!("max_tool_calls: {}\n", m));
            }
            frontmatter.push_str("---\n");
            frontmatter.push_str(prompt);

            fs::write(&filepath, &frontmatter)
                .map_err(|e| format!("Failed to write heartbeat: {}", e))?;

            Ok(format!("Created heartbeat '{}' (schedule: {})", safe_name, schedule))
        }
        "delete_heartbeat" => {
            let name = args["name"]
                .as_str()
                .ok_or("Missing 'name' argument")?;

            let safe_name: String = name
                .chars()
                .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
                .take(64)
                .collect();

            let dir = get_heartbeats_dir(app_handle)?;
            let filepath = dir.join(format!("{}.md", safe_name));
            if !filepath.exists() {
                return Err(format!("Heartbeat '{}' not found", safe_name));
            }

            fs::remove_file(&filepath)
                .map_err(|e| format!("Failed to delete heartbeat: {}", e))?;

            Ok(format!("Deleted heartbeat '{}'", safe_name))
        }
        "edit_heartbeat" => {
            let name = args["name"]
                .as_str()
                .ok_or("Missing 'name' argument")?;

            let safe_name: String = name
                .chars()
                .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
                .take(64)
                .collect();

            let dir = get_heartbeats_dir(app_handle)?;
            let filepath = dir.join(format!("{}.md", safe_name));
            if !filepath.exists() {
                return Err(format!("Heartbeat '{}' not found", safe_name));
            }

            // Read existing spec
            let content = fs::read_to_string(&filepath)
                .map_err(|e| format!("Failed to read heartbeat: {}", e))?;
            let mut spec = parse_heartbeat_spec(&content, &safe_name)?;

            // Apply edits
            if let Some(s) = args["schedule"].as_str() {
                spec.schedule = s.to_string();
            }
            if let Some(p) = args["prompt"].as_str() {
                spec.prompt = p.to_string();
            }
            if let Some(p) = args["persona"].as_str() {
                spec.persona = if p.is_empty() { None } else { Some(p.to_string()) };
            }
            if let Some(m) = args["max_tool_calls"].as_u64() {
                spec.max_tool_calls = m as u32;
            }

            // Rewrite the file
            let mut output = format!(
                "---\nschedule: \"{}\"\nsession: \"{}\"\n",
                spec.schedule, spec.session
            );
            if let Some(ref p) = spec.persona {
                output.push_str(&format!("persona: \"{}\"\n", p));
            }
            if spec.max_tool_calls != 5 {
                output.push_str(&format!("max_tool_calls: {}\n", spec.max_tool_calls));
            }
            output.push_str("---\n");
            output.push_str(&spec.prompt);

            fs::write(&filepath, &output)
                .map_err(|e| format!("Failed to write heartbeat: {}", e))?;

            Ok(format!("Updated heartbeat '{}'", safe_name))
        }
        _ => Err(format!("Unknown draft-gated tool: {}", tool_name)),
    }
}

/// Create a draft proactive message for a gated tool call and insert it into the queue.
/// Returns the draft's proactive message ID.
pub fn create_draft_for_tool_call<R: Runtime>(
    app_handle: &AppHandle<R>,
    spec: &HeartbeatSpec,
    tool_name: &str,
    tool_args: &serde_json::Value,
    justification: &str,
) -> Result<String, String> {
    let draft = DraftPayload {
        name: tool_name.to_string(),
        arguments: tool_args.clone(),
        justification: justification.to_string(),
        heartbeat_session: spec.session.clone(),
    };

    let draft_json = serde_json::to_string(&draft)
        .map_err(|e| format!("Failed to serialize draft: {}", e))?;

    let content = format!(
        "**Heartbeat `{}` requests approval for `{}`:**\n\n{}",
        spec.session, tool_name, justification
    );

    let msg = ProactiveMessage {
        id: uuid::Uuid::new_v4().to_string(),
        heartbeat_session: spec.session.clone(),
        content,
        draft_payload: Some(draft_json),
        needs_approval: true,
        reviewed_at: None,
        approved: None,
        created_at: Utc::now().to_rfc3339(),
    };

    let msg_id = msg.id.clone();
    insert_proactive_message(app_handle, &msg)?;
    app_handle.emit("proactive-message", &msg).ok();

    log::info!(
        "[DraftAct] Created draft for tool '{}' in heartbeat '{}' (msg_id: {})",
        tool_name,
        spec.session,
        msg_id
    );

    Ok(msg_id)
}
