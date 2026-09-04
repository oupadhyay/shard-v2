/**
 * Background Jobs Module
 *
 * Handles periodic maintenance tasks using LLM-powered analysis:
 * - Summary: Analyze recent interactions, extract topics, update summaries
 * - Cleanup: LLM-filter generic/redundant entries from interaction logs
 *
 * Both jobs run sequentially every 6 hours (Summary first, then Cleanup).
 *
 * Note: User-defined scheduled tasks (heartbeats) are handled by the
 * separate heartbeat engine in heartbeat.rs — not here.
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
    #[serde(default)]
    deriver_last_run: Option<String>,
    #[serde(default)]
    dream_last_run: Option<String>,
    #[serde(default)]
    crystals_last_run: Option<String>,
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

/// Result of observation extraction (deriver pipeline)
#[derive(Debug, Serialize, Clone)]
pub struct ExtractionResult {
    pub sessions_processed: usize,
    pub observations_created: usize,
    pub llm_reasoning: Option<String>,
}

/// Result of the dream phase (deduction + induction)
#[derive(Debug, Serialize, Clone)]
pub struct DreamResult {
    pub deductions_created: usize,
    pub inductions_created: usize,
    pub contradictions_found: usize,
    pub peer_card_updated: bool,
    pub llm_reasoning: Option<String>,
}

/// Individual fact extracted by the deriver LLM
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ExtractedFact {
    pub fact: String,
    /// Optional session ID it was extracted from
    #[serde(default)]
    pub session: Option<String>,
}

/// LLM response for observation extraction
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DeriverResponse {
    pub facts: Vec<ExtractedFact>,
}

/// Individual deduction from the dream phase
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DreamDeduction {
    pub content: String,
    pub source_ids: Vec<String>,
    /// "deductive", "inductive", or "contradiction"
    pub level: String,
}

/// LLM response for the dream phase
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct DreamResponse {
    pub observations: Vec<DreamDeduction>,
    #[serde(default)]
    pub peer_card_facts: Vec<String>,
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

// ============================================================================
// Rate Limit Handling
// ============================================================================

/// Maximum number of retries for rate-limited requests.
const RATE_LIMIT_MAX_RETRIES: u32 = 3;

/// Parse a rate limit error message and extract the wait time in seconds.
/// Handles Groq/OpenRouter error formats:
/// - "Please try again in 18.48s"
/// - "Please try again in 1m30s"
/// - "Rate limit" without specific wait → returns a default backoff
///
/// Returns `None` if the error is not a rate limit error.
pub fn parse_rate_limit_wait(error: &str) -> Option<f64> {
    let lower = error.to_lowercase();

    // Must be a rate limit error
    if !lower.contains("rate_limit") && !lower.contains("rate limit") && !lower.contains("429") {
        return None;
    }

    // Try to extract "try again in Xs" or "try again in XmYs"
    if let Some(idx) = lower.find("try again in ") {
        let after = &error[idx + "try again in ".len()..];
        // Parse "18.48s" or "1m30s" or "2m" or "90s"
        let mut seconds = 0.0f64;
        let mut num_buf = String::new();

        for c in after.chars() {
            if c.is_ascii_digit() || c == '.' {
                num_buf.push(c);
            } else if c == 'm' {
                if let Ok(mins) = num_buf.parse::<f64>() {
                    seconds += mins * 60.0;
                }
                num_buf.clear();
            } else if c == 's' {
                if let Ok(secs) = num_buf.parse::<f64>() {
                    seconds += secs;
                }
                break;
            } else {
                break;
            }
        }

        if seconds > 0.0 {
            // Add a small buffer to ensure the limit resets
            return Some(seconds + 1.0);
        }
    }

    // Check for daily/request limit (non-retryable within a short window)
    if lower.contains("per day") || lower.contains("requests per") {
        return None; // Don't retry daily limits
    }

    // Generic rate limit without specific wait time — use 30s default
    Some(30.0)
}

/// Make an LLM call for background processing
/// Routes to the appropriate provider based on the model name
pub async fn call_background_llm(
    http_client: &reqwest::Client,
    config: &crate::config::AppConfig,
    model: &str,
    prompt: &str,
) -> Result<String, String> {
    call_llm_oneshot(
        http_client,
        config,
        model,
        "You are a memory management assistant. Analyze interaction logs and provide structured JSON responses. Be concise and accurate.",
        prompt,
        2000,
        0.3,
    )
    .await
}

/// General-purpose one-shot LLM call with a custom system prompt.
///
/// Uses the background model provider (OpenRouter/Groq).
/// Automatically retries on rate limit errors with the wait time from the error message.
/// Returns the text content of the first response choice.
pub async fn call_llm_oneshot(
    http_client: &reqwest::Client,
    config: &crate::config::AppConfig,
    model: &str,
    system_prompt: &str,
    user_message: &str,
    max_tokens: u32,
    temperature: f64,
) -> Result<String, String> {
    let mut last_err = String::new();

    for attempt in 0..=RATE_LIMIT_MAX_RETRIES {
        if attempt > 0 {
            log::info!("[LLM] Retry attempt {}/{}", attempt, RATE_LIMIT_MAX_RETRIES);
        }

        match call_llm_oneshot_inner(
            http_client,
            config,
            model,
            system_prompt,
            user_message,
            max_tokens,
            temperature,
        )
        .await
        {
            Ok(result) => return Ok(result),
            Err(e) => {
                if let Some(wait_secs) = parse_rate_limit_wait(&e) {
                    if attempt < RATE_LIMIT_MAX_RETRIES {
                        log::info!(
                            "[LLM] Rate limited, waiting {:.1}s before retry...",
                            wait_secs
                        );
                        tokio::time::sleep(Duration::from_secs_f64(wait_secs)).await;
                        last_err = e;
                        continue;
                    }
                }
                return Err(e);
            }
        }
    }

    Err(last_err)
}

/// Inner one-shot LLM call (no retry logic).
/// Routes Gemini-native models through the Gemini REST API (generateContent),
/// all others through the OpenAI-compatible chat/completions path.
async fn call_llm_oneshot_inner(
    http_client: &reqwest::Client,
    config: &crate::config::AppConfig,
    model: &str,
    system_prompt: &str,
    user_message: &str,
    max_tokens: u32,
    temperature: f64,
) -> Result<String, String> {
    // Gemini-native models (gemini-*, gemma-*) go through the Gemini REST API
    if crate::models::is_gemini_model(model) {
        return call_gemini_oneshot(
            http_client,
            config,
            model,
            system_prompt,
            user_message,
            max_tokens,
            temperature,
        )
        .await;
    }

    let (provider_config, api_key) = config.get_model_provider_config(model, "background jobs")?;
    let provider_url = if provider_config.provider_name == "Groq" {
        crate::endpoints::groq_chat()
    } else {
        crate::endpoints::openrouter_chat()
    };
    let transport_config = crate::agent::OpenAiChatTransportConfig {
        endpoint_url: provider_url,
        auth_token: api_key,
    };

    let provider_messages = vec![
        crate::llm_provider::ProviderMessage {
            role: "system".to_string(),
            content: Some(system_prompt.to_string()),
            reasoning: None,
            tool_calls: None,
            tool_call_id: None,
            images: None,
        },
        crate::llm_provider::ProviderMessage {
            role: "user".to_string(),
            content: Some(user_message.to_string()),
            reasoning: None,
            tool_calls: None,
            tool_call_id: None,
            images: None,
        },
    ];
    let provider_request = crate::llm_provider::ProviderChatRequest {
        model: provider_config.model_id,
        messages: provider_messages,
        tools: None,
        tool_choice: None,
        options: crate::llm_provider::ProviderGenerationOptions {
            temperature: Some(temperature),
            max_output_tokens: Some(max_tokens),
            ..Default::default()
        },
        stream: false,
    };
    let request_body = crate::agent::build_chat_completion_request(&provider_request);

    let res =
        crate::agent::send_chat_completion_request(http_client, &transport_config, &request_body)
            .await
            .map_err(|e| format!("Background LLM API network error: {}", e))?;

    if !res.status().is_success() {
        let error_text = res.text().await.unwrap_or_default();
        return Err(format!("Background LLM API error: {}", error_text));
    }

    let body: serde_json::Value = res.json().await.map_err(|e| {
        format!(
            "Failed to parse {} response: {}",
            provider_config.provider_name, e
        )
    })?;

    if let Some(content) = crate::agent::extract_chat_completion_text(&body) {
        return Ok(content);
    }

    Err(format!(
        "No content in {} response",
        provider_config.provider_name
    ))
}

/// Gemini-native one-shot call via the Interactions API.
/// Uses the same `/v1beta/interactions` endpoint as the chat path for consistency,
/// with `stream: false` for a synchronous response.
async fn call_gemini_oneshot(
    http_client: &reqwest::Client,
    config: &crate::config::AppConfig,
    model: &str,
    system_prompt: &str,
    user_message: &str,
    max_tokens: u32,
    temperature: f64,
) -> Result<String, String> {
    let api_key = config
        .gemini_api_key
        .as_deref()
        .ok_or("No Gemini API key configured for background jobs")?;
    let transport_config = crate::agent::GeminiInteractionsTransportConfig {
        endpoint_url: crate::endpoints::gemini_interactions(),
        auth_token: api_key.to_string(),
        api_revision: crate::agent::GEMINI_API_REVISION,
    };
    let request_body = crate::agent::InteractionsRequest {
        model: model.to_string(),
        input: serde_json::json!(user_message),
        system_instruction: Some(system_prompt.to_string()),
        tools: None,
        generation_config: Some(crate::agent::InteractionsGenerationConfig {
            thinking_level: None,
            thinking_summaries: None,
            temperature: Some(temperature as f32),
            max_output_tokens: Some(max_tokens),
        }),
        stream: false,
        store: Some(false),
    };

    let res =
        crate::agent::send_interactions_request(http_client, &transport_config, &request_body)
            .await
            .map_err(|e| format!("Gemini background API network error: {}", e))?;

    if !res.status().is_success() {
        let error_text = res.text().await.unwrap_or_default();
        return Err(format!("Gemini background API error: {}", error_text));
    }

    let body: serde_json::Value = res
        .json()
        .await
        .map_err(|e| format!("Failed to parse Gemini Interactions response: {}", e))?;

    if let Some(text) = crate::agent::extract_interactions_text(&body) {
        return Ok(text);
    }

    Err("No text content in Gemini Interactions response".to_string())
}

/// Response from a tool-aware LLM call.
#[derive(Debug, Clone)]
pub struct LlmToolResponse {
    /// Text content from the assistant (may be empty if model only returns tool calls)
    pub content: Option<String>,
    /// Tool calls requested by the model
    pub tool_calls: Vec<LlmToolCall>,
    /// finish_reason from the API ("stop", "tool_calls", etc.)
    #[allow(dead_code)]
    pub finish_reason: String,
}

#[derive(Debug, Clone)]
pub struct LlmToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String, // raw JSON string
}

/// Call an LLM with tool definitions.
/// Automatically selects the correct API format:
/// - Gemini models: native `generateContent` with `functionDeclarations`
/// - OpenAI-compatible (Groq/OpenRouter): standard `chat/completions` with `tools`
///
/// Automatically retries on rate limit errors with the wait time from the error message.
pub async fn call_llm_with_tools(
    http_client: &reqwest::Client,
    config: &crate::config::AppConfig,
    model: &str,
    messages: &[crate::llm_provider::ProviderMessage],
    tools: &[crate::tool_api::ToolDefinition],
    max_tokens: u32,
    temperature: f64,
) -> Result<LlmToolResponse, String> {
    let mut last_err = String::new();

    for attempt in 0..=RATE_LIMIT_MAX_RETRIES {
        if attempt > 0 {
            log::info!(
                "[LLM] Tool call retry attempt {}/{}",
                attempt,
                RATE_LIMIT_MAX_RETRIES
            );
        }

        let result = if crate::models::is_gemini_model(model) {
            call_gemini_with_tools(
                http_client,
                config,
                model,
                messages,
                tools,
                max_tokens,
                temperature,
            )
            .await
        } else {
            call_openai_with_tools(
                http_client,
                config,
                model,
                messages,
                tools,
                max_tokens,
                temperature,
            )
            .await
        };

        match result {
            Ok(response) => return Ok(response),
            Err(e) => {
                if let Some(wait_secs) = parse_rate_limit_wait(&e) {
                    if attempt < RATE_LIMIT_MAX_RETRIES {
                        log::info!(
                            "[LLM] Rate limited (tools), waiting {:.1}s before retry...",
                            wait_secs
                        );
                        tokio::time::sleep(Duration::from_secs_f64(wait_secs)).await;
                        last_err = e;
                        continue;
                    }
                }
                return Err(e);
            }
        }
    }

    Err(last_err)
}

/// Gemini native generateContent path with functionDeclarations.
async fn call_gemini_with_tools(
    http_client: &reqwest::Client,
    config: &crate::config::AppConfig,
    model: &str,
    messages: &[crate::llm_provider::ProviderMessage],
    tools: &[crate::tool_api::ToolDefinition],
    _max_tokens: u32,
    _temperature: f64,
) -> Result<LlmToolResponse, String> {
    let api_key = config
        .gemini_api_key
        .as_deref()
        .ok_or("No Gemini API key configured for heartbeat")?;
    let transport_config = crate::agent::GeminiGenerateContentTransportConfig {
        endpoint_url: crate::endpoints::gemini_generate_content(model),
        auth_token: api_key.to_string(),
    };
    let provider_tools: Vec<_> = tools
        .iter()
        .map(crate::agent::adapters::provider_tool_definition_from_host)
        .collect();
    let provider_request = crate::llm_provider::ProviderChatRequest {
        model: model.to_string(),
        messages: messages.to_vec(),
        tools: Some(provider_tools),
        tool_choice: None,
        options: crate::llm_provider::ProviderGenerationOptions::default(),
        stream: false,
    };
    let request_body = crate::agent::build_generate_content_request(&provider_request);

    let res =
        crate::agent::send_generate_content_request(http_client, &transport_config, &request_body)
            .await
            .map_err(|e| format!("Gemini heartbeat API error: {}", e))?;

    if !res.status().is_success() {
        let error_text = res.text().await.unwrap_or_default();
        return Err(format!("Gemini heartbeat API error: {}", error_text));
    }

    let body: serde_json::Value = res
        .json()
        .await
        .map_err(|e| format!("Failed to parse Gemini response: {}", e))?;
    let completion = crate::agent::parse_generate_content_completion(&body);

    Ok(LlmToolResponse {
        content: completion.content,
        tool_calls: completion
            .tool_calls
            .into_iter()
            .map(|call| LlmToolCall {
                id: call.id,
                name: call.function.name,
                arguments: call.function.arguments,
            })
            .collect(),
        finish_reason: completion.finish_reason,
    })
}

/// OpenAI-compatible chat/completions path (Groq/OpenRouter).
async fn call_openai_with_tools(
    http_client: &reqwest::Client,
    config: &crate::config::AppConfig,
    model: &str,
    messages: &[crate::llm_provider::ProviderMessage],
    tools: &[crate::tool_api::ToolDefinition],
    max_tokens: u32,
    temperature: f64,
) -> Result<LlmToolResponse, String> {
    let (provider_config, api_key) = config.get_model_provider_config(model, "heartbeat")?;
    let provider_url = if provider_config.provider_name == "Groq" {
        crate::endpoints::groq_chat()
    } else {
        crate::endpoints::openrouter_chat()
    };
    let transport_config = crate::agent::OpenAiChatTransportConfig {
        endpoint_url: provider_url,
        auth_token: api_key,
    };
    let provider_tools: Vec<_> = tools
        .iter()
        .map(crate::agent::adapters::provider_tool_definition_from_host)
        .collect();
    let provider_request = crate::llm_provider::ProviderChatRequest {
        model: provider_config.model_id,
        messages: messages.to_vec(),
        tools: Some(provider_tools),
        tool_choice: Some("auto".to_string()),
        options: crate::llm_provider::ProviderGenerationOptions {
            temperature: Some(temperature),
            max_output_tokens: Some(max_tokens),
            ..Default::default()
        },
        stream: false,
    };
    let request_body = crate::agent::build_chat_completion_request(&provider_request);

    let res =
        crate::agent::send_chat_completion_request(http_client, &transport_config, &request_body)
            .await
            .map_err(|e| format!("Heartbeat LLM API network error: {}", e))?;

    if !res.status().is_success() {
        let error_text = res.text().await.unwrap_or_default();
        return Err(format!("Heartbeat LLM API error: {}", error_text));
    }

    let body: serde_json::Value = res.json().await.map_err(|e| {
        format!(
            "Failed to parse {} response: {}",
            provider_config.provider_name, e
        )
    })?;
    let completion = crate::agent::parse_chat_completion(&body).map_err(|e| {
        format!(
            "Failed to parse {} response: {}",
            provider_config.provider_name, e
        )
    })?;

    Ok(LlmToolResponse {
        content: completion.content,
        tool_calls: completion
            .tool_calls
            .into_iter()
            .map(|call| LlmToolCall {
                id: call.id,
                name: call.function.name,
                arguments: call.function.arguments,
            })
            .collect(),
        finish_reason: completion.finish_reason,
    })
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

/// Start maintenance background jobs (Summary + Cleanup, 6-hour interval).
/// Heartbeat-based scheduled tasks are handled separately by heartbeat::start_heartbeat_engine.
pub fn start_maintenance_jobs<R: Runtime>(app_handle: AppHandle<R>) {
    let summary_cleanup_handle = app_handle.clone();
    tauri::async_runtime::spawn(async move {
        let mut job_interval = time::interval(Duration::from_secs(JOB_INTERVAL_HOURS * 3600));

        loop {
            job_interval.tick().await;

            log::info!("[Background] Starting scheduled jobs (Summary → Cleanup)...");

            // Load last run info to check if we should skip
            let mut last_run_info = load_last_run_info(&summary_cleanup_handle);
            let now = Utc::now().to_rfc3339();

            // Summary job with skip check
            if should_skip_job(last_run_info.summary_last_run.as_deref()) {
                log::info!(
                    "[Background] Skipping summary job - less than {} hours since last run",
                    (JOB_INTERVAL_HOURS as f64 * SKIP_INTERVAL_FRACTION) as u64
                );
            } else {
                log::info!("[Background] Running summary job...");
                match run_summary_job(&summary_cleanup_handle).await {
                    Ok(result) => {
                        log::info!(
                            "[Summary] Complete. {} interactions analyzed, {} topics updated.",
                            result.total_interactions,
                            result.topics_updated.len()
                        );
                        // Update last run time on success
                        if result.llm_reasoning.is_some() || result.total_interactions == 0 {
                            last_run_info.summary_last_run = Some(now.clone());
                            save_last_run_info(&summary_cleanup_handle, &last_run_info);
                        }

                        if !result.topics_updated.is_empty() || !result.insights_created.is_empty()
                        {
                            log::info!(
                                "[Background] Topics/insights changed, rebuilding chunk index..."
                            );
                            if let Ok(config) = crate::config::load_config(&summary_cleanup_handle)
                            {
                                if let Some(api_key) = config.gemini_api_key {
                                    let http_client = reqwest::Client::new();
                                    match crate::memories::rebuild_chunk_index(
                                        &summary_cleanup_handle,
                                        &http_client,
                                        &api_key,
                                    )
                                    .await
                                    {
                                        Ok(count) => log::info!(
                                            "[Background] Chunk index rebuilt with {} chunks",
                                            count
                                        ),
                                        Err(e) => log::warn!(
                                            "[Background] Failed to rebuild chunk index: {}",
                                            e
                                        ),
                                    }
                                }
                            }
                        }
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
                match run_cleanup_job(&summary_cleanup_handle).await {
                    Ok(result) => {
                        log::info!(
                            "[Cleanup] Complete. Removed {} entries, freed {} bytes.",
                            result.deleted_count,
                            result.bytes_freed
                        );
                        // Update last run time on success
                        if result.llm_reasoning.is_some() {
                            last_run_info.cleanup_last_run = Some(Utc::now().to_rfc3339());
                            save_last_run_info(&summary_cleanup_handle, &last_run_info);
                        }
                    }
                    Err(e) => {
                        log::error!("[Background] Cleanup job failed: {}", e);
                    }
                }
            }

            // Deriver job (observation extraction) with skip check
            if should_skip_job(last_run_info.deriver_last_run.as_deref()) {
                log::info!(
                    "[Background] Skipping deriver job - less than {} hours since last run",
                    (JOB_INTERVAL_HOURS as f64 * SKIP_INTERVAL_FRACTION) as u64
                );
            } else {
                log::info!("[Background] Running deriver job (observation extraction)...");
                match run_deriver_job(&summary_cleanup_handle).await {
                    Ok(result) => {
                        let sessions_processed = result.sessions_processed;
                        let observations_created = result.observations_created;
                        log::debug!(
                            "[Deriver] Complete. {} sessions processed, {} observations created.",
                            sessions_processed,
                            observations_created
                        );
                        if result.llm_reasoning.is_some() || sessions_processed == 0 {
                            last_run_info.deriver_last_run = Some(Utc::now().to_rfc3339());
                            save_last_run_info(&summary_cleanup_handle, &last_run_info);
                        }

                        // Trigger dream phase if enough observations exist and haven't been dreamed recently
                        let should_dream = observations_created >= 5
                            || !should_skip_job(last_run_info.dream_last_run.as_deref());
                        if should_dream && observations_created > 0 {
                            log::debug!(
                                "[Background] Triggering dream phase ({} new observations)...",
                                observations_created
                            );
                            match run_dream_job(&summary_cleanup_handle).await {
                                Ok(dream) => {
                                    log::info!(
                                        "[Dream] Complete. {} deductions, {} inductions, {} contradictions, card_updated={}",
                                        dream.deductions_created, dream.inductions_created,
                                        dream.contradictions_found, dream.peer_card_updated
                                    );
                                    if dream.llm_reasoning.is_some() {
                                        last_run_info.dream_last_run =
                                            Some(Utc::now().to_rfc3339());
                                        save_last_run_info(&summary_cleanup_handle, &last_run_info);
                                    }
                                }
                                Err(e) => log::error!("[Background] Dream job failed: {}", e),
                            }
                        }
                    }
                    Err(e) => {
                        log::error!("[Background] Deriver job failed: {}", e);
                    }
                }
            }

            // Crystals sweep — turn completed sketches into proactive_queue
            // persona drafts. Cheap when there are no eligible sketches; the
            // gating predicate filters on tool-call count + success rate
            // before the LLM call fires.
            if should_skip_job(last_run_info.crystals_last_run.as_deref()) {
                log::debug!(
                    "[Background] Skipping crystals sweep - less than {} hours since last run",
                    (JOB_INTERVAL_HOURS as f64 * SKIP_INTERVAL_FRACTION) as u64
                );
            } else {
                log::info!("[Background] Running crystals sweep...");
                match crate::crystals::sweep_and_queue_drafts(&summary_cleanup_handle).await {
                    Ok(queued) => {
                        log::info!("[Crystals] Queued {} drafted persona(s).", queued);
                        last_run_info.crystals_last_run = Some(Utc::now().to_rfc3339());
                        save_last_run_info(&summary_cleanup_handle, &last_run_info);
                    }
                    Err(e) => log::warn!("[Background] Crystals sweep failed: {}", e),
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
    let background_model = config
        .background_model
        .as_deref()
        .unwrap_or(DEFAULT_BACKGROUND_MODEL);

    // Verify we have the required API key
    let _ = config.get_model_provider_config(background_model, "background jobs")?;

    // Gather interactions from lookback period
    let interactions_dir_clone = interactions_dir.clone();
    let (interactions, stats) = tokio::task::spawn_blocking(move || {
        gather_recent_interactions(&interactions_dir_clone, LOOKBACK_HOURS)
    })
    .await
    .map_err(|e| e.to_string())??;

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

    // Load context from disk (offload blocking I/O)
    let handle = app_handle.clone();
    let (existing_topics, existing_insights, daily_logs, candidates_context) =
        tokio::task::spawn_blocking(move || {
            let topics = load_topic_summaries_context_sync(&handle);
            let insights = load_insight_summaries_context_sync(&handle);

            // Get promotion candidates (insights with >= 3 updates)
            let promotion_candidates =
                crate::memories::get_promotion_candidates(&handle, 3).unwrap_or_default();
            let mut candidates = String::new();
            if !promotion_candidates.is_empty() {
                candidates.push_str("CANDIDATES FOR PROMOTION TO TOPIC (Review these):\n");
                for title in &promotion_candidates {
                    if let Ok(content) = crate::memories::read_insight(&handle, title) {
                        candidates
                            .push_str(&format!("- Title: {}\n  Content: {}\n", title, content));
                    }
                }
            }

            // Read daily logs from pre-compaction flushes (staging area for insights)
            let logs = crate::memories::read_all_daily_logs(&handle).unwrap_or_default();

            (topics, insights, logs, candidates)
        })
        .await
        .map_err(|e| format!("Blocking context load failed: {}", e))?;

    let mut daily_logs_context = String::new();
    let logs_to_archive: Vec<String> = daily_logs.iter().map(|(date, _)| date.clone()).collect();
    if !daily_logs.is_empty() {
        daily_logs_context
            .push_str("\nPRE-COMPACTION EXTRACTED FACTS (process these into insights):\n");
        for (date, content) in &daily_logs {
            daily_logs_context.push_str(&format!("--- {} ---\n{}\n", date, content));
        }
        log::info!("[Summary] Processing {} daily logs", daily_logs.len());
    }

    // Call LLM to extract topics AND insights
    let prompt = format!(
        r#"Analyze these interaction logs from the last {} hours and extract knowledge.

EXISTING TOPIC SUMMARIES (broad categories):
{}

EXISTING INSIGHTS (specific facts/Q&A):
{}

DAILY LOGS (process these into insights):
{}

CANDIDATES FOR PROMOTION TO TOPIC (Review these):
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
9. PRE-COMPACTION FACTS: Process these extracted facts into INSIGHTS (they're already curated)

Return JSON object:
{{
  \"topics\": [{{\"topic\": \"Name\", \"summary\": \"content...\"}}],
  \"insights\": [{{\"title\": \"Specific_Fact_Title\", \"content\": \"detailed explanation...\"}}],
  \"promotions\": [{{\"insight_title\": \"Old_Title\", \"new_topic\": \"New_Topic_Name\"}}]
}}

Return at most 5 topics and 5 insights. Ignore generic greetings/one-off queries.
"#,
        LOOKBACK_HOURS,
        existing_topics,
        existing_insights,
        daily_logs_context,
        candidates_context,
        interactions
    );

    let http_client = reqwest::Client::new();
    let llm_response = call_background_llm(&http_client, &config, background_model, &prompt).await;

    let mut topics_updated = vec![];
    let mut insights_created = vec![];
    let mut insights_promoted = vec![];
    let llm_reasoning = match llm_response {
        Ok(response) => {
            log::debug!("[Summary] LLM response: {}", response);

            let response_clone = response.clone();
            let handle = app_handle.clone();
            let result = tokio::task::spawn_blocking(move || {
                let mut topics = Vec::new();
                let mut insights = Vec::new();
                let mut promoted = Vec::new();

                // Try new combined format first
                match parse_extraction_response(&response_clone) {
                    Ok(extraction) => {
                        // Process topics
                        for update in extraction.topics {
                            match crate::memories::update_topic_summary(
                                &handle,
                                &update.topic,
                                &update.summary,
                            ) {
                                Ok(_) => {
                                    log::info!("[Summary] Updated topic: {}", update.topic);
                                    topics.push(update.topic);
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

                        // Process insights
                        for insight in extraction.insights {
                            match crate::memories::update_insight(
                                &handle,
                                &insight.title,
                                &insight.content,
                            ) {
                                Ok(_) => {
                                    log::info!(
                                        "[Summary] Created/Updated insight: {}",
                                        insight.title
                                    );
                                    insights.push(insight.title);
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

                        // Process promotions (delete old insights)
                        for promotion in extraction.promotions {
                            match crate::memories::delete_insight(&handle, &promotion.insight_title)
                            {
                                Ok(true) => {
                                    log::info!(
                                        "[Summary] Promoted insight {} to topic {}",
                                        promotion.insight_title,
                                        promotion.new_topic
                                    );
                                    promoted.push(promotion.insight_title);
                                }
                                Ok(false) => {
                                    log::warn!(
                                        "[Summary] Insight {} not found for promotion",
                                        promotion.insight_title
                                    );
                                }
                                Err(e) => {
                                    log::warn!(
                                        "[Summary] Failed to delete promoted insight {}: {}",
                                        promotion.insight_title,
                                        e
                                    );
                                }
                            }
                        }
                    }
                    Err(_) => {
                        if let Ok(updates) = parse_topic_updates(&response_clone) {
                            for update in updates {
                                if crate::memories::update_topic_summary(
                                    &handle,
                                    &update.topic,
                                    &update.summary,
                                )
                                .is_ok()
                                {
                                    topics.push(update.topic);
                                }
                            }
                        }
                    }
                }
                (topics, insights, promoted)
            })
            .await
            .map_err(|e| format!("Blocking save failed: {}", e))?;

            topics_updated = result.0;
            insights_created = result.1;
            insights_promoted = result.2;
            Some(response)
        }
        Err(e) => {
            log::warn!("[Summary] LLM call failed, running stats-only: {}", e);
            None
        }
    };

    // Archive processed daily logs (move to archived/ folder)
    if !logs_to_archive.is_empty() && !insights_created.is_empty() {
        let handle = app_handle.clone();
        for date in &logs_to_archive {
            let date_clone = date.clone();
            let h = handle.clone();
            match tokio::task::spawn_blocking(move || {
                crate::memories::archive_daily_log(&h, &date_clone)
            })
            .await
            {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => log::warn!("[Summary] Failed to archive daily log {}: {}", date, e),
                Err(e) => log::error!(
                    "[Summary] Archive daily log task panicked for {}: {}",
                    date,
                    e
                ),
            }
        }
        log::info!("[Summary] Archived {} daily logs", logs_to_archive.len());
    }

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

    // ── Prune stale sessions (0–1 user messages, older than 1 day) ──────────
    // These are sessions that were never meaningfully used (e.g. app launched
    // then closed without chatting). No LLM needed — pure SQL decision.
    if let Ok(store) = crate::memories::get_vector_store(app_handle) {
        let prune_result = tokio::task::spawn_blocking(move || {
            store.with_transaction(|_store, conn| {
                // Delete messages for stale sessions first (prevent orphans)
                conn.execute(
                    "DELETE FROM messages WHERE session_id IN (
                        SELECT s.id FROM sessions s
                        WHERE datetime(s.updated_at) < datetime('now', '-1 day')
                          AND (
                            SELECT COUNT(*) FROM messages m
                            WHERE m.session_id = s.id
                              AND m.role = 'user'
                          ) <= 1
                    )",
                    [],
                )?;

                // Delete the sessions themselves
                let n = conn.execute(
                    "DELETE FROM sessions WHERE id IN (
                        SELECT s.id FROM sessions s
                        WHERE datetime(s.updated_at) < datetime('now', '-1 day')
                          AND (
                            SELECT COUNT(*) FROM messages m
                            WHERE m.session_id = s.id
                              AND m.role = 'user'
                          ) <= 1
                    )",
                    [],
                )?;
                Ok(n)
            })
        })
        .await
        .map_err(|e| format!("Task join error: {}", e))?
        .map_err(|e| format!("Database error: {}", e));

        match prune_result {
            Ok(n) if n > 0 => log::info!(
                "[Cleanup] Pruned {} stale sessions (<= 1 user message, > 1 day old)",
                n
            ),
            Ok(_) => {}
            Err(e) => log::warn!("[Cleanup] Failed to prune stale sessions: {}", e),
        }
    }

    let interactions_dir = app_data_dir.join("interactions");

    let config = crate::config::load_config(app_handle)?;
    let background_model = config
        .background_model
        .as_deref()
        .unwrap_or(DEFAULT_BACKGROUND_MODEL);

    // Verify we have the required API key
    if let Err(e) = config.get_model_provider_config(background_model, "background jobs") {
        log::info!("[Cleanup] {}. Falling back to date-based cleanup.", e);
        let dir = interactions_dir.clone();
        return tokio::task::spawn_blocking(move || {
            cleanup_interactions_in_dir(&dir, LOG_RETENTION_DAYS)
        })
        .await
        .map_err(|e| e.to_string())?;
    }

    // Gather same interactions as summary job
    let interactions_dir_clone = interactions_dir.clone();
    let (interactions, _) = tokio::task::spawn_blocking(move || {
        gather_recent_interactions(&interactions_dir_clone, LOOKBACK_HOURS)
    })
    .await
    .map_err(|e| e.to_string())??;

    if interactions.is_empty() {
        return Ok(CleanupResult {
            deleted_count: 0,
            bytes_freed: 0,
            llm_reasoning: None,
        });
    }

    // Load existing topic summaries for context
    let handle = app_handle.clone();
    let topics_context =
        tokio::task::spawn_blocking(move || load_topic_summaries_context_sync(&handle))
            .await
            .map_err(|e| e.to_string())?;

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
                        let h = app_handle.clone();
                        match tokio::task::spawn_blocking(move || {
                            crate::retrieval::prune_bm25_index(&h, LOG_RETENTION_DAYS, 10000)
                        })
                        .await
                        {
                            Ok(Ok(_)) => {}
                            Ok(Err(e)) => log::warn!("[Cleanup] BM25 prune failed: {}", e),
                            Err(e) => log::error!("[Cleanup] BM25 prune task panicked: {}", e),
                        }
                        return Ok(CleanupResult {
                            deleted_count: 0,
                            bytes_freed: 0,
                            llm_reasoning: Some(decision.reasoning),
                        });
                    }

                    // Remove entries by timestamp
                    let dir = interactions_dir.clone();
                    let ts = decision.to_remove.clone();
                    let (deleted, bytes) =
                        tokio::task::spawn_blocking(move || remove_entries_by_timestamp(&dir, &ts))
                            .await
                            .map_err(|e| e.to_string())??;

                    // Also prune BM25 index
                    let h = app_handle.clone();
                    match tokio::task::spawn_blocking(move || {
                        crate::retrieval::prune_bm25_index(&h, LOG_RETENTION_DAYS, 10000)
                    })
                    .await
                    {
                        Ok(Ok(_)) => {}
                        Ok(Err(e)) => log::warn!("[Cleanup] BM25 prune failed: {}", e),
                        Err(e) => log::error!("[Cleanup] BM25 prune task panicked: {}", e),
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
                    let dir = interactions_dir.clone();
                    let result = tokio::task::spawn_blocking(move || {
                        cleanup_interactions_in_dir(&dir, LOG_RETENTION_DAYS)
                    })
                    .await
                    .map_err(|e| e.to_string())??;
                    // Also prune BM25 index
                    let h = app_handle.clone();
                    match tokio::task::spawn_blocking(move || {
                        crate::retrieval::prune_bm25_index(&h, LOG_RETENTION_DAYS, 10000)
                    })
                    .await
                    {
                        Ok(Ok(_)) => {}
                        Ok(Err(e)) => log::warn!("[Cleanup] BM25 prune failed: {}", e),
                        Err(e) => log::error!("[Cleanup] BM25 prune task panicked: {}", e),
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
            let dir = interactions_dir.clone();
            let result = tokio::task::spawn_blocking(move || {
                cleanup_interactions_in_dir(&dir, LOG_RETENTION_DAYS)
            })
            .await
            .map_err(|e| e.to_string())??;
            // Also prune BM25 index
            let h = app_handle.clone();
            match tokio::task::spawn_blocking(move || {
                crate::retrieval::prune_bm25_index(&h, LOG_RETENTION_DAYS, 10000)
            })
            .await
            {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => log::warn!("[Cleanup] BM25 prune failed: {}", e),
                Err(e) => log::error!("[Cleanup] BM25 prune task panicked: {}", e),
            }
            Ok(result)
        }
    }
}

// ============================================================================
// Deriver Job (Observation Extraction)
// ============================================================================

/// Extract explicit observations from recent session transcripts.
/// Processes sessions updated since the last deriver run.
async fn run_deriver_job<R: Runtime>(
    app_handle: &AppHandle<R>,
) -> Result<ExtractionResult, String> {
    let config = crate::config::load_config(app_handle)?;
    let background_model = config
        .background_model
        .as_deref()
        .unwrap_or(DEFAULT_BACKGROUND_MODEL);
    let _ = config.get_model_provider_config(background_model, "deriver")?;

    let gemini_key = config.gemini_api_key.clone();

    // Determine cutoff: sessions updated since last deriver run (or last 12h)
    let last_run_info = load_last_run_info(app_handle);
    let cutoff = last_run_info
        .deriver_last_run
        .as_deref()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|| Utc::now() - ChronoDuration::hours(LOOKBACK_HOURS));
    let cutoff_str = cutoff.to_rfc3339();

    // Fetch recent session transcripts
    let handle = app_handle.clone();
    let cutoff_str_clone = cutoff_str.clone();
    let transcripts: Vec<(String, String)> = tokio::task::spawn_blocking(move || {
        let store = crate::memories::get_vector_store(&handle)?;
        let mut stmt = store
            .conn
            .prepare(
                "SELECT id, title FROM sessions \
                 WHERE updated_at > ? \
                 ORDER BY updated_at ASC LIMIT 10",
            )
            .map_err(|e| e.to_string())?;

        let session_ids: Vec<(String, String)> = stmt
            .query_map([&cutoff_str_clone], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| e.to_string())?
            .filter_map(|r| r.ok())
            .collect();

        let mut results = Vec::new();
        for (sid, title) in session_ids {
            if let Ok(transcript) = crate::db::sessions::get_session_transcript(&store, &sid) {
                if transcript.len() > 50 {
                    // Skip trivially short sessions
                    results.push((sid, format!("Session: {}\n{}", title, transcript)));
                }
            }
        }
        Ok::<_, String>(results)
    })
    .await
    .map_err(|e| format!("Deriver blocking task panicked: {}", e))??;

    if transcripts.is_empty() {
        log::info!("[Deriver] No new sessions to process since {}", cutoff_str);
        return Ok(ExtractionResult {
            sessions_processed: 0,
            observations_created: 0,
            llm_reasoning: None,
        });
    }

    // Build combined transcript for LLM, respecting token budget.
    // Background models (e.g. gpt-oss-20b on Groq) often have small TPM limits.
    // Rough heuristic: 1 token ≈ 4 chars. Reserve ~1500 tokens for prompt + response.
    const MAX_TRANSCRIPT_CHARS: usize = 20_000; // ~5000 tokens
    let mut combined = String::new();
    for (_, t) in &transcripts {
        if combined.len() + t.len() + 4 > MAX_TRANSCRIPT_CHARS {
            // Truncate the current transcript to fit
            let remaining = MAX_TRANSCRIPT_CHARS.saturating_sub(combined.len() + 4);
            if remaining > 100 {
                if !combined.is_empty() {
                    combined.push_str("\n---\n");
                }
                let boundary = t.floor_char_boundary(remaining);
                combined.push_str(&t[..boundary]);
                combined.push_str("...");
            }
            break;
        }
        if !combined.is_empty() {
            combined.push_str("\n---\n");
        }
        combined.push_str(t);
    }

    let prompt = format!(
        r#"Extract atomic facts about the USER from these chat transcripts.
Focus ONLY on facts the user explicitly stated or clearly implied about themselves.

Rules:
1. Each fact must be a single, self-contained statement
2. Focus on: preferences, biographical info, habits, opinions, technical stack, work details
3. Ignore the assistant's responses unless they confirm a user fact
4. Ignore generic greetings, one-off queries, and ephemeral requests
5. Do NOT extract facts about the assistant or about external topics
6. Deduplicate: if the same fact appears multiple times, include it only once

TRANSCRIPTS:
{}

Return a JSON object:
{{
  "facts": [
    {{"fact": "User prefers Rust for backend development"}},
    {{"fact": "User has a cat named Luna"}}
  ]
}}

Return at most 15 facts. If no user facts are found, return {{"facts": []}}."#,
        combined
    );

    let http_client = reqwest::Client::new();
    let llm_response = call_background_llm(&http_client, &config, background_model, &prompt).await;

    let mut observations_created = 0usize;
    let llm_reasoning = match llm_response {
        Ok(response) => {
            log::debug!("[Deriver] LLM response: {}", response);

            // Parse facts
            let facts = parse_deriver_response(&response);

            if !facts.is_empty() {
                // Pre-filter facts whose content_hash already exists in the store.
                // Doing this BEFORE the embedding API calls avoids wasting Gemini quota
                // (and rate-limit budget) on duplicates that would later be skipped anyway.
                let dedup_handle = app_handle.clone();
                let fact_strings: Vec<String> = facts.iter().map(|f| f.fact.clone()).collect();
                let novel_indices: Vec<usize> = tokio::task::spawn_blocking(move || {
                    match crate::memories::get_vector_store(&dedup_handle) {
                        Ok(store) => fact_strings
                            .iter()
                            .enumerate()
                            .filter_map(|(idx, fact_text)| {
                                let hash = crate::vector_store::compute_content_hash(fact_text);
                                let exists: bool = store
                                    .conn
                                    .query_row(
                                        "SELECT COUNT(*) FROM observations WHERE content_hash = ? AND deleted_at IS NULL",
                                        [&hash],
                                        |row| row.get::<_, i64>(0),
                                    )
                                    .unwrap_or(0) > 0;
                                if exists { None } else { Some(idx) }
                            })
                            .collect(),
                        // If the store can't be opened we'll fall through and try every fact;
                        // the inner spawn_blocking below will surface the error.
                        Err(_) => (0..fact_strings.len()).collect(),
                    }
                })
                .await
                .map_err(|e| format!("Deriver dedup panicked: {}", e))?;

                // Build a sparse embeddings vector aligned with `facts` so the inner
                // spawn_blocking loop can still pair embeddings to facts by index.
                let mut precomputed_embeddings: Vec<Option<Vec<f32>>> = vec![None; facts.len()];

                if !novel_indices.is_empty() {
                    // Pre-compute embeddings asynchronously in parallel for novel facts only.
                    // Wrap the API key in Arc<str> so each task gets a cheap pointer-clone
                    // instead of duplicating the secret string per fact.
                    use futures::StreamExt;
                    use std::sync::Arc;
                    let key_arc: Option<Arc<str>> = gemini_key.as_deref().map(Arc::from);
                    let novel_facts: Vec<(usize, String)> = novel_indices
                        .iter()
                        .map(|&i| (i, facts[i].fact.clone()))
                        .collect();

                    let results: Vec<(usize, Option<Vec<f32>>)> =
                        futures::stream::iter(novel_facts)
                            .map(|(idx, fact_text)| {
                                let client = http_client.clone();
                                let key_opt = key_arc.clone();
                                async move {
                                    let emb = if let Some(key) = key_opt {
                                        let embedding_config = crate::gemini_embedding::GeminiEmbeddingConfig {
                                            endpoint_url: crate::endpoints::gemini_embedding(),
                                            auth_token: key.to_string(),
                                            output_dimensionality: Some(768),
                                        };
                                        match crate::gemini_embedding::generate_embedding(
                                            &client,
                                            &fact_text,
                                            &embedding_config,
                                        )
                                        .await
                                        {
                                            Ok(emb) => Some(emb),
                                            Err(e) => {
                                                log::warn!("[Deriver] Embedding generation failed for fact, will try cache: {}", e);
                                                None
                                            }
                                        }
                                    } else {
                                        None
                                    };
                                    (idx, emb)
                                }
                            })
                            .buffered(4) // 4 concurrent embedding calls
                            .collect()
                            .await;

                    for (idx, emb) in results {
                        precomputed_embeddings[idx] = emb;
                    }
                }

                let handle = app_handle.clone();

                observations_created = tokio::task::spawn_blocking(move || {
                    let store = crate::memories::get_vector_store(&handle)
                        .map_err(|e| format!("Failed to open vector store: {}", e))?;
                    let mut created = 0usize;

                    for (fact, precomputed) in facts.iter().zip(precomputed_embeddings) {
                        // Check for duplicate by content hash
                        let hash = crate::vector_store::compute_content_hash(&fact.fact);
                        let exists: bool = store
                            .conn
                            .query_row(
                                "SELECT COUNT(*) FROM observations WHERE content_hash = ? AND deleted_at IS NULL",
                                [&hash],
                                |row| row.get::<_, i64>(0),
                            )
                            .unwrap_or(0) > 0;

                        if exists {
                            log::debug!("[Deriver] Skipping duplicate: {}", fact.fact);
                            continue;
                        }

                        let obs = crate::observations::make_observation(
                            &fact.fact,
                            crate::observations::ObservationLevel::Explicit,
                            vec![],
                            fact.session.clone(),
                        );

                        // Use pre-computed embedding, fall back to cache
                        let embedding = precomputed.or_else(|| {
                            let hash = crate::vector_store::compute_content_hash(&fact.fact);
                            store.get_cached_embedding(&hash).ok().flatten()
                        });

                        match crate::observations::insert_observation(
                            &store,
                            &obs,
                            embedding.as_deref(),
                        ) {
                            Ok(()) => {
                                log::info!("[Deriver] Created observation: {}", fact.fact);
                                created += 1;
                            }
                            Err(e) => log::warn!("[Deriver] Failed to insert observation: {}", e),
                        }
                    }
                    Ok::<_, String>(created)
                })
                .await
                .map_err(|e| format!("Deriver save panicked: {}", e))?
                .unwrap_or(0);
            }

            Some(response)
        }
        Err(e) => {
            log::warn!("[Deriver] LLM call failed: {}", e);
            None
        }
    };

    Ok(ExtractionResult {
        sessions_processed: transcripts.len(),
        observations_created,
        llm_reasoning,
    })
}

/// Parse the deriver LLM response into extracted facts.
pub fn parse_deriver_response(response: &str) -> Vec<ExtractedFact> {
    let json_start = response.find('{');
    let json_end = response.rfind('}');

    if let (Some(start), Some(end)) = (json_start, json_end) {
        let json_str = &response[start..=end];
        if let Ok(parsed) = serde_json::from_str::<DeriverResponse>(json_str) {
            return parsed.facts;
        }
    }

    Vec::new()
}

// ============================================================================
// Dream Job (Deduction + Induction)
// ============================================================================

/// Analyze existing explicit observations to derive higher-level insights.
/// - Deductions: logical implications from 1+ explicit observations
/// - Inductions: behavioral patterns across multiple observations
/// - Contradictions: flagged conflicts between observations
/// - Peer card: curated biographical summary updated from all levels
async fn run_dream_job<R: Runtime>(app_handle: &AppHandle<R>) -> Result<DreamResult, String> {
    let config = crate::config::load_config(app_handle)?;
    let background_model = config
        .background_model
        .as_deref()
        .unwrap_or(DEFAULT_BACKGROUND_MODEL);
    let _ = config.get_model_provider_config(background_model, "dream")?;

    // Gather recent explicit observations for the dream phase
    let handle = app_handle.clone();
    let (explicit_obs, existing_deductive, existing_inductive) =
        tokio::task::spawn_blocking(move || {
            let store = crate::memories::get_vector_store(&handle)?;
            let explicit = crate::observations::get_observations_by_level(
                &store,
                "user",
                crate::observations::ObservationLevel::Explicit,
                50,
            )?;
            let deductive = crate::observations::get_observations_by_level(
                &store,
                "user",
                crate::observations::ObservationLevel::Deductive,
                20,
            )?;
            let inductive = crate::observations::get_observations_by_level(
                &store,
                "user",
                crate::observations::ObservationLevel::Inductive,
                20,
            )?;
            Ok::<_, String>((explicit, deductive, inductive))
        })
        .await
        .map_err(|e| format!("Dream blocking task panicked: {}", e))??;

    if explicit_obs.is_empty() {
        log::info!("[Dream] No explicit observations to analyze");
        return Ok(DreamResult {
            deductions_created: 0,
            inductions_created: 0,
            contradictions_found: 0,
            peer_card_updated: false,
            llm_reasoning: None,
        });
    }

    // Format observations for LLM
    let mut explicit_text = String::new();
    for obs in &explicit_obs {
        explicit_text.push_str(&format!("- [{}] {}\n", obs.id, obs.content));
    }

    let mut existing_text = String::new();
    if !existing_deductive.is_empty() {
        existing_text.push_str("Existing Deductions:\n");
        for obs in &existing_deductive {
            existing_text.push_str(&format!("- {}\n", obs.content));
        }
    }
    if !existing_inductive.is_empty() {
        existing_text.push_str("Existing Inductions/Patterns:\n");
        for obs in &existing_inductive {
            existing_text.push_str(&format!("- {}\n", obs.content));
        }
    }

    let prompt = format!(
        r#"Analyze these explicit facts about the user and derive higher-level observations.

EXPLICIT OBSERVATIONS (source facts):
{}

EXISTING HIGHER-LEVEL OBSERVATIONS (avoid duplicates):
{}

Tasks:
1. DEDUCTIONS: Identify logical implications from 1+ explicit facts. Include the source observation IDs.
   Example: If user "lives in SF" and "commutes daily" → "User likely commutes in the Bay Area"
2. INDUCTIONS: Identify behavioral patterns across multiple observations.
   Example: If user prefers Rust, uses Tauri, avoids JavaScript → "User favors systems-level typed languages"
3. CONTRADICTIONS: Flag any conflicts between observations.
   Example: "User said they live in SF" vs "User said they live in NYC"
4. PEER CARD: Produce a curated list of 5-10 key biographical facts (one sentence each) for a quick user profile.

Return a JSON object:
{{
  "observations": [
    {{"content": "User likely commutes in Bay Area", "source_ids": ["id1", "id2"], "level": "deductive"}},
    {{"content": "User favors typed systems languages", "source_ids": ["id3", "id4", "id5"], "level": "inductive"}}
  ],
  "peer_card_facts": [
    "Software engineer who prefers Rust",
    "Lives in San Francisco"
  ]
}}

Rules:
- Only create observations that add NEW knowledge not already in the existing set
- Each source_id must reference an actual observation ID from the EXPLICIT list above
- The level field must be "deductive", "inductive", or "contradiction"
- Return at most 10 observations and 10 peer card facts
- If nothing meaningful can be derived, return {{"observations": [], "peer_card_facts": []}}"#,
        explicit_text, existing_text
    );

    let http_client = reqwest::Client::new();
    let llm_response = call_background_llm(&http_client, &config, background_model, &prompt).await;

    let mut deductions_created = 0usize;
    let mut inductions_created = 0usize;
    let mut contradictions_found = 0usize;
    let mut peer_card_updated = false;

    let llm_reasoning = match llm_response {
        Ok(response) => {
            log::debug!("[Dream] LLM response: {}", response);

            let parsed = parse_dream_response(&response);

            if !parsed.observations.is_empty() || !parsed.peer_card_facts.is_empty() {
                let handle = app_handle.clone();
                let dream_data = parsed.clone();

                let result = tokio::task::spawn_blocking(move || {
                    let store = crate::memories::get_vector_store(&handle)?;

                    let mut deductions = 0usize;
                    let mut inductions = 0usize;
                    let mut contradictions = 0usize;

                    for dream_obs in &dream_data.observations {
                        let level = match dream_obs.level.as_str() {
                            "deductive" => crate::observations::ObservationLevel::Deductive,
                            "inductive" => crate::observations::ObservationLevel::Inductive,
                            "contradiction" => {
                                crate::observations::ObservationLevel::Contradiction
                            }
                            _ => continue,
                        };

                        // Deduplicate by content hash
                        let hash = crate::vector_store::compute_content_hash(&dream_obs.content);
                        let exists: bool = store
                            .conn
                            .query_row(
                                "SELECT COUNT(*) FROM observations WHERE content_hash = ? AND deleted_at IS NULL",
                                [&hash],
                                |row| row.get::<_, i64>(0),
                            )
                            .unwrap_or(0) > 0;

                        if exists {
                            continue;
                        }

                        let obs = crate::observations::make_observation(
                            &dream_obs.content,
                            level,
                            dream_obs.source_ids.clone(),
                            None,
                        );

                        match crate::observations::insert_observation(&store, &obs, None) {
                            Ok(()) => {
                                match level {
                                    crate::observations::ObservationLevel::Deductive => {
                                        deductions += 1
                                    }
                                    crate::observations::ObservationLevel::Inductive => {
                                        inductions += 1
                                    }
                                    crate::observations::ObservationLevel::Contradiction => {
                                        contradictions += 1;
                                        // Auto-resolve: soft-delete the older source observations
                                        // that this contradiction supersedes
                                        for source_id in &dream_obs.source_ids {
                                            let _ = crate::observations::soft_delete_observation(&store, source_id);
                                            log::info!("[Dream] Soft-deleted conflicting observation: {}", source_id);
                                        }
                                    }
                                    _ => {}
                                }
                                log::info!("[Dream] Created {:?} observation: {}", level, dream_obs.content);
                            }
                            Err(e) => log::warn!("[Dream] Failed to insert: {}", e),
                        }
                    }

                    // Update peer card
                    let card_updated = if !dream_data.peer_card_facts.is_empty() {
                        crate::observations::upsert_peer_card(
                            &store,
                            "shard",
                            "user",
                            &dream_data.peer_card_facts,
                        )
                        .is_ok()
                    } else {
                        false
                    };

                    Ok::<_, String>((deductions, inductions, contradictions, card_updated))
                })
                .await
                .map_err(|e| format!("Dream save panicked: {}", e))??;

                deductions_created = result.0;
                inductions_created = result.1;
                contradictions_found = result.2;
                peer_card_updated = result.3;
            }

            Some(response)
        }
        Err(e) => {
            log::warn!("[Dream] LLM call failed: {}", e);
            None
        }
    };

    Ok(DreamResult {
        deductions_created,
        inductions_created,
        contradictions_found,
        peer_card_updated,
        llm_reasoning,
    })
}

/// Parse the dream LLM response.
pub fn parse_dream_response(response: &str) -> DreamResponse {
    let json_start = response.find('{');
    let json_end = response.rfind('}');

    if let (Some(start), Some(end)) = (json_start, json_end) {
        let json_str = &response[start..=end];
        if let Ok(parsed) = serde_json::from_str::<DreamResponse>(json_str) {
            return parsed;
        }
    }

    DreamResponse {
        observations: Vec::new(),
        peer_card_facts: Vec::new(),
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
    if result.llm_reasoning.is_some() || result.total_interactions == 0 {
        let mut last_run_info = load_last_run_info(app_handle);
        last_run_info.summary_last_run = Some(Utc::now().to_rfc3339());
        save_last_run_info(app_handle, &last_run_info);
    }

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
    if result.llm_reasoning.is_some() {
        let mut last_run_info = load_last_run_info(app_handle);
        last_run_info.cleanup_last_run = Some(Utc::now().to_rfc3339());
        save_last_run_info(app_handle, &last_run_info);
    }

    Ok(result)
}

/// Force-trigger the deriver job (observation extraction)
pub async fn force_deriver<R: Runtime>(
    app_handle: &AppHandle<R>,
) -> Result<ExtractionResult, String> {
    log::info!("[Background] Force-triggered deriver job");
    let result = run_deriver_job(app_handle).await?;

    if result.llm_reasoning.is_some() || result.sessions_processed == 0 {
        let mut last_run_info = load_last_run_info(app_handle);
        last_run_info.deriver_last_run = Some(Utc::now().to_rfc3339());
        save_last_run_info(app_handle, &last_run_info);
    }

    Ok(result)
}

/// Force-trigger the dream phase (deduction + induction)
pub async fn force_dream<R: Runtime>(app_handle: &AppHandle<R>) -> Result<DreamResult, String> {
    log::info!("[Background] Force-triggered dream job");
    let result = run_dream_job(app_handle).await?;

    if result.llm_reasoning.is_some() {
        let mut last_run_info = load_last_run_info(app_handle);
        last_run_info.dream_last_run = Some(Utc::now().to_rfc3339());
        save_last_run_info(app_handle, &last_run_info);
    }

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
            #[allow(clippy::lines_filter_map_ok)]
            for line in reader.lines().filter_map(Result::ok) {
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

/// Load topic summaries as context string (sync version for blocking tasks)
fn load_topic_summaries_context_sync<R: Runtime>(app_handle: &AppHandle<R>) -> String {
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

/// Load insight summaries as context string for background job (sync version)
fn load_insight_summaries_context_sync<R: Runtime>(app_handle: &AppHandle<R>) -> String {
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

            #[allow(clippy::lines_filter_map_ok)]
            for line in reader.lines().filter_map(Result::ok) {
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
