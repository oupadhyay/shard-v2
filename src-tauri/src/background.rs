/**
 * Background Jobs Module
 *
 * Handles periodic maintenance tasks using LLM-powered analysis:
 * - Summary: Analyze recent interactions, extract topics, update summaries
 * - Cleanup: LLM-filter generic/redundant entries from interaction logs
 *
 * Both jobs run sequentially every 6 hours (Summary first, then Cleanup).
 */
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use tauri::{AppHandle, Manager, Runtime};
use tokio::time::{self, Duration};

/// Configuration for background jobs
pub const JOB_INTERVAL_HOURS: u64 = 6;
pub const LOOKBACK_HOURS: i64 = 12;
pub const LOG_RETENTION_DAYS: i64 = 30; // Fallback for date-based cleanup
/// Default background model if none configured
pub const DEFAULT_BACKGROUND_MODEL: &str = "gpt-oss-120b (Groq)";
/// Skip job execution if less than this fraction of the interval has passed
const SKIP_INTERVAL_FRACTION: f64 = 0.5;

// ============================================================================
// Last Run Persistence
// ============================================================================

/// Stores the last run timestamps for background jobs
#[derive(Debug, Serialize, Deserialize, Default)]
struct LastRunInfo {
    summary_last_run: Option<String>,
    cleanup_last_run: Option<String>,
}

/// Get the path to the last_run.json file
fn get_last_run_path<R: Runtime>(app_handle: &AppHandle<R>) -> Result<PathBuf, String> {
    let app_data_dir = app_handle
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to get app data dir: {}", e))?;
    Ok(app_data_dir.join("last_run.json"))
}

/// Load the last run info from disk
fn load_last_run_info<R: Runtime>(app_handle: &AppHandle<R>) -> LastRunInfo {
    match get_last_run_path(app_handle) {
        Ok(path) => {
            if path.exists() {
                match fs::read_to_string(&path) {
                    Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
                    Err(_) => LastRunInfo::default(),
                }
            } else {
                LastRunInfo::default()
            }
        }
        Err(_) => LastRunInfo::default(),
    }
}

/// Save the last run info to disk
fn save_last_run_info<R: Runtime>(app_handle: &AppHandle<R>, info: &LastRunInfo) {
    if let Ok(path) = get_last_run_path(app_handle) {
        if let Ok(content) = serde_json::to_string_pretty(info) {
            let _ = fs::write(&path, content);
        }
    }
}

/// Check if we should skip a job based on last run time
/// Returns true if less than half the interval has passed since last run
fn should_skip_job(last_run_str: Option<&str>) -> bool {
    let Some(last_run_str) = last_run_str else {
        return false; // No previous run, should execute
    };

    let last_run = match DateTime::parse_from_rfc3339(last_run_str) {
        Ok(dt) => dt.with_timezone(&Utc),
        Err(_) => return false, // Invalid timestamp, run the job
    };

    let now = Utc::now();
    let elapsed = now.signed_duration_since(last_run);
    let skip_threshold_hours = (JOB_INTERVAL_HOURS as f64 * SKIP_INTERVAL_FRACTION) as i64;
    let skip_threshold = ChronoDuration::hours(skip_threshold_hours);

    elapsed < skip_threshold
}

// ============================================================================
// Result Types
// ============================================================================

/// Result of cleanup operation
#[derive(Debug, PartialEq, Serialize, Clone)]
pub struct CleanupResult {
    pub deleted_count: usize,
    pub bytes_freed: u64,
    pub llm_reasoning: Option<String>,
}

/// Result of summary analysis
#[derive(Debug, PartialEq, Serialize, Clone)]
pub struct SummaryResult {
    pub total_interactions: usize,
    pub user_messages: usize,
    pub assistant_messages: usize,
    pub total_chars: usize,
    pub topics_updated: Vec<String>,
    pub insights_created: Vec<String>,
    pub insights_promoted: Vec<String>,
    pub llm_reasoning: Option<String>,
}

/// Topic extraction from LLM
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct TopicUpdate {
    pub topic: String,
    pub summary: String,
}

/// Insight extraction from LLM (niche Q&A pairs)
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct InsightExtraction {
    pub title: String,
    pub content: String,
}

/// Promotion of insight to topic
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Promotion {
    pub insight_title: String,
    pub new_topic: String,
}

/// Combined extraction response from LLM
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ExtractionResponse {
    pub topics: Vec<TopicUpdate>,
    pub insights: Vec<InsightExtraction>,
    #[serde(default)]
    pub promotions: Vec<Promotion>,
}

/// Cleanup decision from LLM
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CleanupDecision {
    pub to_remove: Vec<String>, // timestamps or IDs of entries to remove
    pub reasoning: String,
}

// ============================================================================
// LLM Integration
// ============================================================================

/// Make an LLM call for background processing
/// Routes to Groq or Cerebras based on the model name
async fn call_background_llm(
    http_client: &reqwest::Client,
    config: &crate::config::AppConfig,
    model: &str,
    prompt: &str,
) -> Result<String, String> {
    // Parse model to determine provider and model ID
    let (url, api_key, model_id) = if model.contains("(Cerebras)") {
        let key = config.cerebras_api_key.as_ref()
            .ok_or("No Cerebras API key configured for background jobs")?;
        let model_id = if model.contains("120b") {
            "gpt-oss-120b"
        } else {
            "llama-3.3-70b"
        };
        ("https://api.cerebras.ai/v1/chat/completions", key, model_id)
    } else if model.contains("(OpenRouter)") {
        let key = config.openrouter_api_key.as_ref()
            .ok_or("No OpenRouter API key configured for background jobs")?;
        // Extract model ID from format like "google/gemma-3-27b-it:free (OpenRouter)"
        let model_id = model.split(" (OpenRouter)").next().unwrap_or("google/gemma-3-27b-it:free");
        ("https://openrouter.ai/api/v1/chat/completions", key, model_id.to_string().leak() as &str)
    } else {
        // Default to Groq
        let key = config.groq_api_key.as_ref()
            .ok_or("No Groq API key configured for background jobs")?;
        let model_id = if model.contains("20b") {
            "openai/gpt-oss-20b"
        } else {
            "openai/gpt-oss-120b"
        };
        ("https://api.groq.com/openai/v1/chat/completions", key, model_id)
    };

    let payload = serde_json::json!({
        "model": model_id,
        "messages": [
            {
                "role": "system",
                "content": "You are a memory management assistant. Analyze interaction logs and provide structured JSON responses. Be concise and accurate."
            },
            {
                "role": "user",
                "content": prompt
            }
        ],
        "temperature": 0.3,
        "max_tokens": 2000
    });

    let res = http_client
        .post(url)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("Background LLM API network error: {}", e))?;

    if !res.status().is_success() {
        let error_text = res.text().await.unwrap_or_default();
        return Err(format!("Background LLM API error: {}", error_text));
    }

    let body: serde_json::Value = res
        .json()
        .await
        .map_err(|e| format!("Failed to parse Groq response: {}", e))?;

    // Extract text content from response
    if let Some(choices) = body.get("choices").and_then(|c| c.as_array()) {
        if let Some(first) = choices.first() {
            if let Some(content) = first
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_str())
            {
                return Ok(content.to_string());
            }
        }
    }

    Err("No content in Groq response".to_string())
}

/// Parse topic updates from LLM JSON response
pub fn parse_topic_updates(llm_response: &str) -> Result<Vec<TopicUpdate>, String> {
    // Try to find JSON array in response (LLM might include extra text)
    let json_start = llm_response.find('[');
    let json_end = llm_response.rfind(']');

    if let (Some(start), Some(end)) = (json_start, json_end) {
        let json_str = &llm_response[start..=end];
        serde_json::from_str(json_str).map_err(|e| format!("Failed to parse topic updates: {}", e))
    } else {
        Err("No JSON array found in LLM response".to_string())
    }
}

/// Parse combined extraction response (topics + insights) from LLM JSON
pub fn parse_extraction_response(llm_response: &str) -> Result<ExtractionResponse, String> {
    // Try to find JSON object in response
    let json_start = llm_response.find('{');
    let json_end = llm_response.rfind('}');

    if let (Some(start), Some(end)) = (json_start, json_end) {
        let json_str = &llm_response[start..=end];
        serde_json::from_str(json_str)
            .map_err(|e| format!("Failed to parse extraction response: {}", e))
    } else {
        Err("No JSON object found in LLM response".to_string())
    }
}

/// Parse cleanup decision from LLM JSON response
pub fn parse_cleanup_decision(llm_response: &str) -> Result<CleanupDecision, String> {
    // Try to find JSON object in response
    let json_start = llm_response.find('{');
    let json_end = llm_response.rfind('}');

    if let (Some(start), Some(end)) = (json_start, json_end) {
        let json_str = &llm_response[start..=end];
        serde_json::from_str(json_str)
            .map_err(|e| format!("Failed to parse cleanup decision: {}", e))
    } else {
        Err("No JSON object found in LLM response".to_string())
    }
}

// ============================================================================
// Background Job Runner
// ============================================================================

/// Start all background jobs (sequential: Summary first, then Cleanup)
pub fn start_background_jobs<R: Runtime>(app_handle: AppHandle<R>) {
    tauri::async_runtime::spawn(async move {
        let mut job_interval = time::interval(Duration::from_secs(JOB_INTERVAL_HOURS * 3600));

        loop {
            job_interval.tick().await;

            log::info!("[Background] Starting scheduled jobs (Summary → Cleanup)...");

            // Load last run info to check if we should skip
            let mut last_run_info = load_last_run_info(&app_handle);
            let now = Utc::now().to_rfc3339();

            // Summary job with skip check
            if should_skip_job(last_run_info.summary_last_run.as_deref()) {
                log::info!(
                    "[Background] Skipping summary job - less than {} hours since last run",
                    (JOB_INTERVAL_HOURS as f64 * SKIP_INTERVAL_FRACTION) as u64
                );
            } else {
                log::info!("[Background] Running summary job...");
                match run_summary_job(&app_handle).await {
                    Ok(result) => {
                        log::info!(
                            "[Summary] Complete. {} interactions analyzed, {} topics updated.",
                            result.total_interactions,
                            result.topics_updated.len()
                        );
                        // Update last run time on success
                        last_run_info.summary_last_run = Some(now.clone());
                        save_last_run_info(&app_handle, &last_run_info);
                    }
                    Err(e) => {
                        log::error!("[Background] Summary job failed: {}", e);
                    }
                }
            }

            // Cleanup job with skip check
            if should_skip_job(last_run_info.cleanup_last_run.as_deref()) {
                log::info!(
                    "[Background] Skipping cleanup job - less than {} hours since last run",
                    (JOB_INTERVAL_HOURS as f64 * SKIP_INTERVAL_FRACTION) as u64
                );
            } else {
                log::info!("[Background] Running cleanup job...");
                match run_cleanup_job(&app_handle).await {
                    Ok(result) => {
                        log::info!(
                            "[Cleanup] Complete. Removed {} entries, freed {} bytes.",
                            result.deleted_count,
                            result.bytes_freed
                        );
                        // Update last run time on success
                        last_run_info.cleanup_last_run = Some(Utc::now().to_rfc3339());
                        save_last_run_info(&app_handle, &last_run_info);
                    }
                    Err(e) => {
                        log::error!("[Background] Cleanup job failed: {}", e);
                    }
                }
            }

            log::info!(
                "[Background] All jobs complete. Next run in {} hours.",
                JOB_INTERVAL_HOURS
            );
        }
    });
}

// ============================================================================
// Summary Job
// ============================================================================

/// Analyze recent interactions and update topic summaries using LLM
async fn run_summary_job<R: Runtime>(app_handle: &AppHandle<R>) -> Result<SummaryResult, String> {
    let app_data_dir = app_handle
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to get app data dir: {}", e))?;

    let interactions_dir = app_data_dir.join("interactions");

    let config = crate::config::load_config(app_handle)?;
    let background_model = config.background_model.as_deref()
        .unwrap_or(DEFAULT_BACKGROUND_MODEL);

    // Verify we have the required API key
    if background_model.contains("(Cerebras)") {
        config.cerebras_api_key.as_ref()
            .ok_or("No Cerebras API key configured for background jobs")?;
    } else if background_model.contains("(OpenRouter)") {
        config.openrouter_api_key.as_ref()
            .ok_or("No OpenRouter API key configured for background jobs")?;
    } else {
        config.groq_api_key.as_ref()
            .ok_or("No Groq API key configured for background jobs")?;
    };

    // Gather interactions from lookback period
    let (interactions, stats) = gather_recent_interactions(&interactions_dir, LOOKBACK_HOURS)?;

    if interactions.is_empty() {
        log::info!("[Summary] No interactions in lookback period.");
        return Ok(SummaryResult {
            total_interactions: 0,
            user_messages: 0,
            assistant_messages: 0,
            total_chars: 0,
            topics_updated: vec![],
            insights_created: vec![],
            insights_promoted: vec![],
            llm_reasoning: None,
        });
    }

    // Load existing topic summaries so LLM can update/merge them
    let existing_topics = load_topic_summaries_context(app_handle);
    let existing_insights = load_insight_summaries_context(app_handle);

    // Get promotion candidates (insights with >= 3 updates)
    let promotion_candidates = crate::memories::get_promotion_candidates(app_handle, 3).unwrap_or_default();
    let mut candidates_context = String::new();
    if !promotion_candidates.is_empty() {
        candidates_context.push_str("CANDIDATES FOR PROMOTION TO TOPIC (Review these):\n");
        for title in &promotion_candidates {
            if let Ok(content) = crate::memories::read_insight(app_handle, title) {
                candidates_context.push_str(&format!("- Title: {}\n  Content: {}\n", title, content));
            }
        }
    }

    // Call LLM to extract topics AND insights
    let prompt = format!(
        r#"Analyze these interaction logs from the last {} hours and extract knowledge.

EXISTING TOPIC SUMMARIES (broad categories):
{}

EXISTING INSIGHTS (specific facts/Q&A):
{}

{}

NEW INTERACTIONS TO ANALYZE:
{}

INSTRUCTIONS:
1. TOPICS are BROAD categories (e.g., \"Preferences\", \"Hardware\", \"Career\", project names)
2. INSIGHTS are SPECIFIC facts or Q&A pairs that are too narrow for topics but worth remembering
   Examples of insights:
   - \"Tauri 2.0 requires dylib bundling for macOS distribution\"
   - \"User's M3 Pro has 36GB RAM\"
   - \"vitest uses jsdom environment for tests\"
3. TOPIC SCOPE RULES (CRITICAL):
   - Each topic has a SPECIFIC DOMAIN. Only add info that directly relates to its title.
   - About_Me = personal bio only (name, age, birthday, pronouns, interests)
   - Hardware = devices/specs only
   - Preferences = likes/dislikes only
   - Career = job/education only
   - DO NOT merge travel, health, relationships, or other domains into About_Me
   - If info doesn't fit an existing topic's domain, create a NEW topic or insight
4. If info relates to an existing topic's domain, UPDATE that topic
5. If info is too specific for a topic, create an INSIGHT
6. Use underscores in names (e.g., \"Tauri_macOS_Distribution\")
7. PRIORITY: User-stated facts override assistant responses
8. UP-LEVELING: Review the \"CANDIDATES FOR PROMOTION\". If an insight has enough distinct info to be a broad topic:
   - Create/Update the TOPIC with the insight's content
   - Add a \"promotions\" entry to delete the old insight

Return JSON object:
{{
  \"topics\": [{{\"topic\": \"Name\", \"summary\": \"content...\"}}],
  \"insights\": [{{\"title\": \"Specific_Fact_Title\", \"content\": \"detailed explanation...\"}}],
  \"promotions\": [{{\"insight_title\": \"Old_Title\", \"new_topic\": \"New_Topic_Name\"}}]
}}

Return at most 5 topics and 5 insights. Ignore generic greetings/one-off queries.
"#,
        LOOKBACK_HOURS, existing_topics, existing_insights, candidates_context, interactions
    );

    let http_client = reqwest::Client::new();
    let llm_response = call_background_llm(&http_client, &config, background_model, &prompt).await;

    let mut topics_updated = vec![];
    let mut insights_created = vec![];
    let mut insights_promoted = vec![];
    let llm_reasoning = match llm_response {
        Ok(response) => {
            log::debug!("[Summary] LLM response: {}", response);

            // Try new combined format first
            match parse_extraction_response(&response) {
                Ok(extraction) => {
                    let gemini_api_key = config.gemini_api_key.as_ref();

                    // Process topics
                    for update in extraction.topics {
                        if let Some(api_key) = gemini_api_key {
                            match crate::memories::update_topic_summary(
                                app_handle,
                                &http_client,
                                api_key,
                                &update.topic,
                                &update.summary,
                            )
                            .await
                            {
                                Ok(_) => {
                                    log::info!("[Summary] Updated topic: {}", update.topic);
                                    topics_updated.push(update.topic);
                                }
                                Err(e) => {
                                    log::warn!(
                                        "[Summary] Failed to update topic {}: {}",
                                        update.topic,
                                        e
                                    );
                                }
                            }
                        }
                    }

                    // Process insights
                    for insight in extraction.insights {
                        if let Some(api_key) = gemini_api_key {
                            match crate::memories::update_insight(
                                app_handle,
                                &http_client,
                                api_key,
                                &insight.title,
                                &insight.content,
                            )
                            .await
                            {
                                Ok(_) => {
                                    log::info!("[Summary] Created/Updated insight: {}", insight.title);
                                    insights_created.push(insight.title);
                                }
                                Err(e) => {
                                    log::warn!(
                                        "[Summary] Failed to create insight {}: {}",
                                        insight.title,
                                        e
                                    );
                                }
                            }
                        }
                    }

                    // Process promotions (delete old insights)
                    for promotion in extraction.promotions {
                        match crate::memories::delete_insight(app_handle, &promotion.insight_title) {
                            Ok(true) => {
                                log::info!("[Summary] Promoted insight {} to topic {}", promotion.insight_title, promotion.new_topic);
                                insights_promoted.push(promotion.insight_title);
                            }
                            Ok(false) => {
                                log::warn!("[Summary] Failed to find insight to promote: {}", promotion.insight_title);
                            }
                            Err(e) => {
                                log::warn!("[Summary] Error promoting insight {}: {}", promotion.insight_title, e);
                            }
                        }
                    }
                }
                Err(e) => {
                    // Fallback: try old topic-only format
                    log::debug!(
                        "[Summary] Combined format failed ({}), trying legacy format",
                        e
                    );
                    if let Ok(updates) = parse_topic_updates(&response) {
                        let gemini_api_key = config.gemini_api_key.as_ref();
                        for update in updates {
                            if let Some(api_key) = gemini_api_key {
                                if let Ok(_) = crate::memories::update_topic_summary(
                                    app_handle,
                                    &http_client,
                                    api_key,
                                    &update.topic,
                                    &update.summary,
                                )
                                .await
                                {
                                    topics_updated.push(update.topic);
                                }
                            }
                        }
                    }
                }
            }
            Some(response)
        }
        Err(e) => {
            log::warn!("[Summary] LLM call failed, running stats-only: {}", e);
            None
        }
    };

    // TODO: Up-leveling phase - check insights with reference_count >= INSIGHT_UPLEVEL_THRESHOLD
    // and merge/promote them to topics

    Ok(SummaryResult {
        total_interactions: stats.total_interactions,
        user_messages: stats.user_messages,
        assistant_messages: stats.assistant_messages,
        total_chars: stats.total_chars,
        topics_updated,
        insights_created,
        insights_promoted,
        llm_reasoning,
    })
}

// ============================================================================
// Cleanup Job
// ============================================================================

/// Clean up redundant interaction entries using LLM judgment
async fn run_cleanup_job<R: Runtime>(app_handle: &AppHandle<R>) -> Result<CleanupResult, String> {
    let app_data_dir = app_handle
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to get app data dir: {}", e))?;

    let interactions_dir = app_data_dir.join("interactions");

    let config = crate::config::load_config(app_handle)?;
    let background_model = config.background_model.as_deref()
        .unwrap_or(DEFAULT_BACKGROUND_MODEL);

    // Verify we have the required API key
    let has_key = if background_model.contains("(Cerebras)") {
        config.cerebras_api_key.is_some()
    } else if background_model.contains("(OpenRouter)") {
        config.openrouter_api_key.is_some()
    } else {
        config.groq_api_key.is_some()
    };

    if !has_key {
        log::info!("[Cleanup] No API key for {}, falling back to date-based cleanup", background_model);
        return cleanup_interactions_in_dir(&interactions_dir, LOG_RETENTION_DAYS);
    }

    // Gather same interactions as summary job
    let (interactions, _) = gather_recent_interactions(&interactions_dir, LOOKBACK_HOURS)?;

    if interactions.is_empty() {
        return Ok(CleanupResult {
            deleted_count: 0,
            bytes_freed: 0,
            llm_reasoning: None,
        });
    }

    // Load existing topic summaries for context
    let topics_context = load_topic_summaries_context(app_handle);

    // Call LLM to decide what to clean up
    let prompt = format!(
        r#"Given these topic summaries and the same interaction entries just analyzed, identify which entries:
1. Are generic greetings/one-off questions with no reusable context
2. Have their key information now captured in the updated topic summaries
3. Should be retained for future context

Return JSON: {{"to_remove": [list of entry timestamps], "reasoning": "explanation"}}

Be conservative - when in doubt, keep entries.

Topic Summaries:
{}

Interaction Entries:
{}
"#,
        topics_context, interactions
    );

    let http_client = reqwest::Client::new();
    let llm_response = call_background_llm(&http_client, &config, background_model, &prompt).await;

    match llm_response {
        Ok(response) => {
            log::debug!("[Cleanup] LLM response: {}", response);

            match parse_cleanup_decision(&response) {
                Ok(decision) => {
                    if decision.to_remove.is_empty() {
                        // Also prune BM25 index
                        if let Err(e) = crate::retrieval::prune_bm25_index(
                            app_handle,
                            LOG_RETENTION_DAYS,
                            10000,
                        ) {
                            log::warn!("[Cleanup] BM25 prune failed: {}", e);
                        }
                        return Ok(CleanupResult {
                            deleted_count: 0,
                            bytes_freed: 0,
                            llm_reasoning: Some(decision.reasoning),
                        });
                    }

                    // Remove entries by timestamp
                    let (deleted, bytes) =
                        remove_entries_by_timestamp(&interactions_dir, &decision.to_remove)?;

                    // Also prune BM25 index
                    if let Err(e) =
                        crate::retrieval::prune_bm25_index(app_handle, LOG_RETENTION_DAYS, 10000)
                    {
                        log::warn!("[Cleanup] BM25 prune failed: {}", e);
                    }

                    Ok(CleanupResult {
                        deleted_count: deleted,
                        bytes_freed: bytes,
                        llm_reasoning: Some(decision.reasoning),
                    })
                }
                Err(e) => {
                    log::warn!(
                        "[Cleanup] Failed to parse LLM response: {}. Using date-based fallback.",
                        e
                    );
                    let result =
                        cleanup_interactions_in_dir(&interactions_dir, LOG_RETENTION_DAYS)?;
                    // Also prune BM25 index
                    if let Err(e) =
                        crate::retrieval::prune_bm25_index(app_handle, LOG_RETENTION_DAYS, 10000)
                    {
                        log::warn!("[Cleanup] BM25 prune failed: {}", e);
                    }
                    Ok(result)
                }
            }
        }
        Err(e) => {
            log::warn!(
                "[Cleanup] LLM call failed: {}. Using date-based fallback.",
                e
            );
            let result = cleanup_interactions_in_dir(&interactions_dir, LOG_RETENTION_DAYS)?;
            // Also prune BM25 index
            if let Err(e) =
                crate::retrieval::prune_bm25_index(app_handle, LOG_RETENTION_DAYS, 10000)
            {
                log::warn!("[Cleanup] BM25 prune failed: {}", e);
            }
            Ok(result)
        }
    }
}

// ============================================================================
// Force Trigger Commands
// ============================================================================

/// Force-trigger the summary job (public API for on-demand analysis)
/// Also updates the last run timestamp to prevent redundant scheduled runs
pub async fn force_summary<R: Runtime>(app_handle: &AppHandle<R>) -> Result<SummaryResult, String> {
    log::info!("[Background] Force-triggered summary job");
    let result = run_summary_job(app_handle).await?;

    // Update last run time on success
    let mut last_run_info = load_last_run_info(app_handle);
    last_run_info.summary_last_run = Some(Utc::now().to_rfc3339());
    save_last_run_info(app_handle, &last_run_info);

    Ok(result)
}

/// Run memory refresh from agent tool call
/// Alias for force_summary - provides semantic clarity when called from agent context
pub async fn run_summary_job_from_agent<R: Runtime>(
    app_handle: &AppHandle<R>,
) -> Result<SummaryResult, String> {
    log::info!("[Background] Agent-initiated memory refresh");
    force_summary(app_handle).await
}

/// Force-trigger the cleanup job (public API for on-demand cleanup)
/// Also updates the last run timestamp to prevent redundant scheduled runs
pub async fn force_cleanup<R: Runtime>(app_handle: &AppHandle<R>) -> Result<CleanupResult, String> {
    log::info!("[Background] Force-triggered cleanup job");
    let result = run_cleanup_job(app_handle).await?;

    // Update last run time on success
    let mut last_run_info = load_last_run_info(app_handle);
    last_run_info.cleanup_last_run = Some(Utc::now().to_rfc3339());
    save_last_run_info(app_handle, &last_run_info);

    Ok(result)
}

// ============================================================================
// Helper Functions
// ============================================================================

struct InteractionStats {
    total_interactions: usize,
    user_messages: usize,
    assistant_messages: usize,
    total_chars: usize,
}

/// Gather recent interactions as formatted text for LLM
fn gather_recent_interactions(
    interactions_dir: &std::path::Path,
    lookback_hours: i64,
) -> Result<(String, InteractionStats), String> {
    if !interactions_dir.exists() {
        return Ok((
            String::new(),
            InteractionStats {
                total_interactions: 0,
                user_messages: 0,
                assistant_messages: 0,
                total_chars: 0,
            },
        ));
    }

    let cutoff = Utc::now() - ChronoDuration::hours(lookback_hours);
    let cutoff_str = cutoff.format("%Y-%m-%d").to_string();
    let today_str = Utc::now().format("%Y-%m-%d").to_string();

    let mut output = String::new();
    let mut stats = InteractionStats {
        total_interactions: 0,
        user_messages: 0,
        assistant_messages: 0,
        total_chars: 0,
    };

    let entries = fs::read_dir(interactions_dir)
        .map_err(|e| format!("Failed to read interactions dir: {}", e))?;

    for entry in entries.flatten() {
        let path = entry.path();

        if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
            continue;
        }

        // Check if file date is within lookback window
        if let Some(filename) = path.file_stem().and_then(|s| s.to_str()) {
            if let Some(date_str) = filename.strip_prefix("interactions-") {
                if date_str < cutoff_str.as_str() && date_str != today_str {
                    continue;
                }
            }
        }

        if let Ok(file) = fs::File::open(&path) {
            let reader = BufReader::new(file);
            for line in reader.lines().flatten() {
                if let Ok(entry) = serde_json::from_str::<serde_json::Value>(&line) {
                    stats.total_interactions += 1;

                    let role = entry
                        .get("role")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown");
                    let content = entry.get("content").and_then(|v| v.as_str()).unwrap_or("");
                    let ts = entry.get("ts").and_then(|v| v.as_str()).unwrap_or("");

                    match role {
                        "user" => stats.user_messages += 1,
                        "assistant" | "model" => stats.assistant_messages += 1,
                        _ => {}
                    }
                    stats.total_chars += content.len();

                    // Format for LLM (truncate long content, respecting UTF-8 boundaries)
                    let truncated = if content.len() > 500 {
                        // Find valid UTF-8 boundary at or before byte 500
                        let boundary = content.floor_char_boundary(500);
                        format!("{}...", &content[..boundary])
                    } else {
                        content.to_string()
                    };
                    output.push_str(&format!("[{}] {}: {}\n", ts, role, truncated));
                }
            }
        }
    }

    Ok((output, stats))
}

/// Load topic summaries as context string
fn load_topic_summaries_context<R: Runtime>(app_handle: &AppHandle<R>) -> String {
    match crate::memories::get_topics_dir(app_handle) {
        Ok(topics_dir) => {
            if !topics_dir.exists() {
                return "No topic summaries yet.".to_string();
            }

            let mut context = String::new();
            if let Ok(entries) = fs::read_dir(&topics_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|s| s.to_str()) == Some("md") {
                        if let Some(topic) = path.file_stem().and_then(|s| s.to_str()) {
                            if let Ok(content) = fs::read_to_string(&path) {
                                // Truncate long summaries (respecting UTF-8 boundaries)
                                let truncated = if content.len() > 1000 {
                                    let boundary = content.floor_char_boundary(1000);
                                    format!("{}...", &content[..boundary])
                                } else {
                                    content
                                };
                                context.push_str(&format!("### {}\n{}\n\n", topic, truncated));
                            }
                        }
                    }
                }
            }

            if context.is_empty() {
                "No topic summaries yet.".to_string()
            } else {
                context
            }
        }
        Err(_) => "No topic summaries yet.".to_string(),
    }
}

/// Load insight summaries as context string for background job
fn load_insight_summaries_context<R: Runtime>(app_handle: &AppHandle<R>) -> String {
    match crate::memories::get_insights_dir(app_handle) {
        Ok(insights_dir) => {
            if !insights_dir.exists() {
                return "No insights yet.".to_string();
            }

            let mut context = String::new();
            if let Ok(entries) = fs::read_dir(&insights_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|s| s.to_str()) == Some("md") {
                        if let Some(title) = path.file_stem().and_then(|s| s.to_str()) {
                            if let Ok(content) = fs::read_to_string(&path) {
                                // Truncate long insights
                                let truncated = if content.len() > 500 {
                                    let boundary = content.floor_char_boundary(500);
                                    format!("{}...", &content[..boundary])
                                } else {
                                    content
                                };
                                context.push_str(&format!(
                                    "- {}: {}\n",
                                    title,
                                    truncated.replace('\n', " ")
                                ));
                            }
                        }
                    }
                }
            }

            if context.is_empty() {
                "No insights yet.".to_string()
            } else {
                context
            }
        }
        Err(_) => "No insights yet.".to_string(),
    }
}

/// Remove specific entries by timestamp from JSONL files
fn remove_entries_by_timestamp(
    interactions_dir: &std::path::Path,
    timestamps: &[String],
) -> Result<(usize, u64), String> {
    if !interactions_dir.exists() || timestamps.is_empty() {
        return Ok((0, 0));
    }

    let mut deleted_count = 0;
    let mut bytes_freed = 0u64;

    let entries = fs::read_dir(interactions_dir)
        .map_err(|e| format!("Failed to read interactions dir: {}", e))?;

    for entry in entries.flatten() {
        let path = entry.path();

        if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
            continue;
        }

        // Read file, filter entries, rewrite
        if let Ok(file) = fs::File::open(&path) {
            let reader = BufReader::new(file);
            let mut kept_lines = Vec::new();
            let mut removed_in_file = 0;

            for line in reader.lines().flatten() {
                if let Ok(entry) = serde_json::from_str::<serde_json::Value>(&line) {
                    let ts = entry.get("ts").and_then(|v| v.as_str()).unwrap_or("");

                    if timestamps.iter().any(|t| ts.contains(t)) {
                        removed_in_file += 1;
                        bytes_freed += line.len() as u64 + 1; // +1 for newline
                    } else {
                        kept_lines.push(line);
                    }
                } else {
                    kept_lines.push(line); // Keep unparseable lines
                }
            }

            if removed_in_file > 0 {
                // Rewrite file with kept lines
                let file = OpenOptions::new()
                    .write(true)
                    .truncate(true)
                    .open(&path)
                    .map_err(|e| format!("Failed to rewrite interaction file: {}", e))?;

                let mut writer = std::io::BufWriter::new(file);
                for line in kept_lines {
                    writeln!(writer, "{}", line)
                        .map_err(|e| format!("Failed to write line: {}", e))?;
                }

                deleted_count += removed_in_file;
            }
        }
    }

    Ok((deleted_count, bytes_freed))
}

// ============================================================================
// Fallback Date-Based Cleanup (Testable Core Logic)
// ============================================================================

/// Core cleanup logic operating on a directory path directly (testable)
/// Used as fallback when LLM is unavailable
pub fn cleanup_interactions_in_dir(
    interactions_dir: &std::path::Path,
    retention_days: i64,
) -> Result<CleanupResult, String> {
    if !interactions_dir.exists() {
        return Ok(CleanupResult {
            deleted_count: 0,
            bytes_freed: 0,
            llm_reasoning: None,
        });
    }

    let cutoff_date = Utc::now() - ChronoDuration::days(retention_days);
    let cutoff_str = cutoff_date.format("%Y-%m-%d").to_string();

    let mut deleted_count = 0;
    let mut bytes_freed = 0u64;

    let entries = fs::read_dir(interactions_dir)
        .map_err(|e| format!("Failed to read interactions dir: {}", e))?;

    for entry in entries.flatten() {
        let path = entry.path();

        if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
            continue;
        }

        if let Some(filename) = path.file_stem().and_then(|s| s.to_str()) {
            if let Some(date_str) = filename.strip_prefix("interactions-") {
                if date_str < cutoff_str.as_str() {
                    if let Ok(metadata) = fs::metadata(&path) {
                        bytes_freed += metadata.len();
                    }

                    if fs::remove_file(&path).is_ok() {
                        deleted_count += 1;
                    }
                }
            }
        }
    }

    Ok(CleanupResult {
        deleted_count,
        bytes_freed,
        llm_reasoning: None,
    })
}

/// Core summary analysis logic operating on a directory path directly (testable)
#[allow(dead_code)]
pub fn analyze_interactions_in_dir(
    interactions_dir: &std::path::Path,
    lookback_hours: i64,
) -> Result<SummaryResult, String> {
    let (_, stats) = gather_recent_interactions(interactions_dir, lookback_hours)?;

    Ok(SummaryResult {
        total_interactions: stats.total_interactions,
        user_messages: stats.user_messages,
        assistant_messages: stats.assistant_messages,
        total_chars: stats.total_chars,
        topics_updated: vec![],
        insights_created: vec![],
        insights_promoted: vec![],
        llm_reasoning: None,
    })
}
