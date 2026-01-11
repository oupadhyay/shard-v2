/**
 * Agent module - AI chat agent with Gemini and OpenRouter support
 */
mod gemini;
mod openrouter;
mod types;

pub use gemini::{construct_gemini_messages, parse_gemini_chunk, AgentEvent};
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
    backup_history: Mutex<Option<Vec<ChatMessage>>>,
    data_dir: std::path::PathBuf,
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

        // Load persisted history if it exists
        let history_path = app_data_dir.join("chat_history.json");
        let history = if history_path.exists() {
            match std::fs::read_to_string(&history_path) {
                Ok(contents) => match serde_json::from_str::<Vec<ChatMessage>>(&contents) {
                    Ok(msgs) => {
                        log::info!("Loaded {} messages from persisted history", msgs.len());
                        msgs
                    }
                    Err(e) => {
                        log::warn!("Failed to parse chat history: {}", e);
                        Vec::new()
                    }
                },
                Err(e) => {
                    log::warn!("Failed to read chat history: {}", e);
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };

        Self {
            history: Mutex::new(history),
            http_client,
            uploaded_files: Mutex::new(Vec::new()),
            backup_history: Mutex::new(None),
            data_dir: app_data_dir,
        }
    }

    pub async fn clear_history(&self, api_key: Option<String>) {
        let mut history = self.history.lock().await;
        history.clear();

        let mut uploaded_files = self.uploaded_files.lock().await;
        if !uploaded_files.is_empty() {
            if let Some(key) = api_key {
                for uri in uploaded_files.iter() {
                    if let Some(file_name) = uri.split('/').last() {
                        let delete_url = format!(
                            "https://generativelanguage.googleapis.com/v1beta/files/{}?key={}",
                            file_name, key
                        );
                        let _ = self.http_client.delete(&delete_url).send().await;
                    }
                }
            }
            uploaded_files.clear();
        }

        // Persist the cleared state
        drop(history); // Release lock before persist
        drop(uploaded_files);
        self.persist_history().await;
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
    }

    pub async fn save_and_clear_history(&self) {
        let mut history = self.history.lock().await;
        let mut backup = self.backup_history.lock().await;
        *backup = Some(history.clone());
        history.clear();
    }

    pub async fn restore_history(&self) -> Result<(), String> {
        let mut history = self.history.lock().await;
        let mut backup = self.backup_history.lock().await;

        if let Some(saved) = backup.take() {
            *history = saved;
            Ok(())
        } else {
            Err("No backup available".to_string())
        }
    }

    pub async fn get_history(&self) -> Vec<ChatMessage> {
        let history = self.history.lock().await;
        history.clone()
    }

    pub async fn get_message_count(&self) -> usize {
        let history = self.history.lock().await;
        history.len()
    }

    pub async fn has_backup(&self) -> bool {
        let backup = self.backup_history.lock().await;
        backup.is_some()
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
                let hint = RetryReason::MalformedLatex { errors: katex_errors }.get_hint();
                history.push(ChatMessage {
                    role: "user".to_string(),
                    content: Some(hint),
                    reasoning: None,
                    tool_calls: None,
                    tool_call_id: None,
                    images: None,
                });

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

        let stream_id = crate::CURRENT_STREAM_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;

        let selected_model = config
            .selected_model
            .clone()
            .unwrap_or("gemini-2.5-flash-lite".to_string());

        let is_gemini = !selected_model.contains("/")
            && !selected_model.contains("(Cerebras)")
            && !selected_model.contains("(Groq)");

        let _continue_turn = if is_gemini {
            let api_key = config.gemini_api_key.as_ref().ok_or("No Gemini API key")?;
            self.process_gemini_turn(
                app_handle,
                config,
                &mut history,
                stream_id,
                &selected_model,
                api_key,
                None, // No RAG context for retry
                false, // Not research mode
            )
            .await?
        } else {
            self.process_openrouter_turn(
                app_handle,
                config,
                &mut history,
                stream_id,
                None,
                false,
            )
            .await?
        };

        // Persist the new response
        drop(history);
        self.persist_history().await;

        Ok(())
    }

    /// Persist current chat history to disk
    pub async fn persist_history(&self) {
        let history = self.history.lock().await;
        let history_path = self.data_dir.join("chat_history.json");

        match serde_json::to_string_pretty(&*history) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&history_path, json) {
                    log::error!("Failed to persist chat history: {}", e);
                }
            }
            Err(e) => {
                log::error!("Failed to serialize chat history: {}", e);
            }
        }
    }

    pub async fn process_message<R: Runtime>(
        &self,
        app_handle: &AppHandle<R>,
        message: String,
        images_base64: Option<Vec<String>>,
        images_mime_types: Option<Vec<String>>,
        config: &crate::config::AppConfig,
    ) -> Result<(), String> {
        println!("process_message called. Message len: {}", message.len());

        let mut history = self.history.lock().await;

        // Determine model type
        let selected_model = config
            .selected_model
            .clone()
            .unwrap_or("gemini-2.5-flash-lite".to_string());
        let is_gemini = !selected_model.contains("/");

        // Process images: upload to Gemini Files API if using Gemini model,
        // or describe via Vision LLM for other providers
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
                        // Upload to Gemini Files API
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
                    } else {
                        // For non-Gemini providers, use Vision LLM to describe the image
                        match crate::integrations::vision_llm::describe_image(
                            &self.http_client,
                            img_data,
                            mime_type,
                            config,
                        )
                        .await
                        {
                            Ok(description) => {
                                log::info!("[Agent] Vision LLM described image: {} chars", description.len());
                                image_descriptions.push(description);
                            }
                            Err(e) => {
                                log::warn!("[Agent] Vision LLM failed: {}", e);
                                image_descriptions.push("[Image attached but could not be described]".to_string());
                            }
                        }
                        None // No file URI for non-Gemini
                    };

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

        // For non-Gemini providers, prepend image descriptions to the message
        let augmented_message = if !is_gemini && !image_descriptions.is_empty() {
            let descriptions = image_descriptions.join("\n\n");
            format!("[Image Description]\n{}\n\n[User Message]\n{}", descriptions, message)
        } else {
            message.clone()
        };

        history.push(ChatMessage {
            role: "user".to_string(),
            content: Some(augmented_message),
            reasoning: None,
            tool_calls: None,
            tool_call_id: None,
            images: uploaded_images,
        });

        // RAG: Generate embedding and retrieve relevant interactions using hybrid search (BM25 + Dense + RRF)
        let user_embedding = if let Some(api_key) = &config.gemini_api_key {
            crate::interactions::generate_embedding(&self.http_client, &message, api_key)
                .await
                .ok()
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

        // RAG: Context from Topics or Insights (Tier 2 / 2.5)
        if let Some(emb) = &user_embedding {
            if let Ok(Some((name, content, is_insight))) =
                crate::memories::find_relevant_context(app_handle, emb)
            {
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

            // Detect provider: Gemini models don't have slash or provider suffixes
            let is_gemini = !selected_model.contains("/")
                && !selected_model.contains("(Cerebras)")
                && !selected_model.contains("(Groq)");

            // Inject retry hint if pending (from previous failed attempt)
            if let Some(hint) = pending_retry_hint.take() {
                history.push(ChatMessage {
                    role: "user".to_string(),
                    content: Some(hint),
                    reasoning: None,
                    tool_calls: None,
                    tool_call_id: None,
                    images: None,
                });
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
                    let has_reasoning = last_msg.reasoning.as_ref().map(|r| !r.is_empty()).unwrap_or(false);
                    let has_content = last_msg.content.as_ref().map(|c| !c.trim().is_empty()).unwrap_or(false);
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

            if !continue_turn {
                break;
            }
        }

        // Log interactions for future RAG (skip in incognito mode)
        let incognito = config.incognito_mode.unwrap_or(false);

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

            // Persist history to disk after each message exchange
            drop(history); // Release lock before persist
            self.persist_history().await;
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
            "save_memory" => {
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
                let topic = args["topic"].as_str().unwrap_or_default();
                let content = args["content"].as_str().unwrap_or_default();
                if let Some(api_key) = config.gemini_api_key.as_ref() {
                    match crate::memories::update_topic_summary(
                        app_handle,
                        &self.http_client,
                        api_key,
                        topic,
                        content,
                    )
                    .await
                    {
                        Ok(_) => format!("Topic summary updated: {}", topic),
                        Err(e) => format!("Failed to update topic summary: {}", e),
                    }
                } else {
                    "Failed: No Gemini API key available for embedding generation".to_string()
                }
            }
            "read_topic_summary" => {
                let topic = args["topic"].as_str().unwrap_or_default();
                match crate::memories::read_topic_summary(app_handle, topic) {
                    Ok(content) => content,
                    Err(e) => format!("Failed to read topic summary: {}", e),
                }
            }
            _ => format!("Unknown tool: {}", function_name),
        }
    }

    async fn classify_intent(&self, query: &str, api_key: &str) -> Result<bool, String> {
        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash-lite:generateContent?key={}",
            api_key
        );

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
            .post(&url)
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
            "https://generativelanguage.googleapis.com/v1beta/models/{}:streamGenerateContent?key={}",
            selected_model, api_key
        );

        // Load memories for injection into system prompt
        let memory_context = crate::memories::get_memories_for_prompt(app_handle)
            .ok()
            .filter(|s| !s.is_empty());

        let system_prompt_content = if config.incognito_mode.unwrap_or(false) {
            crate::prompts::get_jailbreak_prompt(&selected_model)
        } else if is_research_mode {
            crate::prompts::get_research_system_prompt()
        } else {
            config.system_prompt.clone().unwrap_or_else(|| {
                crate::prompts::get_default_system_prompt(memory_context.as_deref(), rag_context)
            })
        };

        let contents = construct_gemini_messages(history);
        let system_instruction = Some(GeminiContent {
            role: None,
            parts: vec![GeminiPart::Text {
                text: system_prompt_content.clone(),
            }],
        });

        let gemini_tools = if enable_tools {
            Some(vec![GeminiTool {
                function_declarations: crate::tools::get_all_tools()
                    .iter()
                    .map(|t| t.function.clone())
                    .collect(),
            }])
        } else {
            None
        };

        let supports_thinking =
            selected_model.contains("2.5") || selected_model.contains("gemini-3") || selected_model.contains("thinking");

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
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send()
            .await
            .map_err(|e| format!("API network error: {}", e))?;

        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_default();
            app_handle.emit("agent-error", format!("Gemini API Error: {}", error_text)).ok();
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
            history.push(ChatMessage {
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
                                arguments: serde_json::to_string(&fc.function_call.args).unwrap_or_default(),
                            },
                            thought_signature: fc.thought_signature.clone(),
                        })
                        .collect(),
                ),
                tool_call_id: None,
                images: None,
            });

            for (idx, fc) in tool_calls.into_iter().enumerate() {
                let function_name = &fc.function_call.name;
                let args = &fc.function_call.args;

                let tool_call_event = json!({
                    "name": function_name,
                    "args": args
                });
                app_handle
                    .emit("agent-tool-call", tool_call_event.to_string())
                    .ok();

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

                history.push(ChatMessage {
                    role: "tool".to_string(),
                    content: Some(tool_result),
                    reasoning: None,
                    tool_calls: None,
                    tool_call_id: Some(format!("call_{}_{}", fc.function_call.name, idx)),
                    images: None,
                });
            }
            Ok(true) // Continue loop so model can respond to tool results
        } else {
            history.push(ChatMessage {
                role: "assistant".to_string(),
                content: Some(full_text),
                reasoning: if full_reasoning.is_empty() {
                    None
                } else {
                    Some(full_reasoning.trim_end().to_string())
                },
                tool_calls: None,
                tool_call_id: None,
                images: None,
            });
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
        let is_cerebras = selected_model.contains("(Cerebras)");
        let is_groq = selected_model.contains("(Groq)");

        let (api_key, base_url, model, reasoning_effort, provider_name) = if is_cerebras {
            // Cerebras: strip suffix and use Cerebras endpoint
            let key = config
                .cerebras_api_key
                .as_ref()
                .ok_or("No Cerebras API key configured")?;
            let clean_model = selected_model.replace(" (Cerebras)", "").trim().to_string();
            (
                key.clone(),
                "https://api.cerebras.ai/v1/".to_string(),
                clean_model,
                Some("high".to_string()), // Cerebras supports reasoning_effort
                "Cerebras",
            )
        } else if is_groq {
            // Groq: strip suffix, add openai/ prefix, and use Groq endpoint
            let key = config
                .groq_api_key
                .as_ref()
                .ok_or("No Groq API key configured")?;
            // Groq expects model names like "openai/gpt-oss-120b"
            let base_model = selected_model.replace(" (Groq)", "").trim().to_string();
            let clean_model = format!("openai/{}", base_model);
            (
                key.clone(),
                "https://api.groq.com/openai/v1/".to_string(),
                clean_model,
                Some("high".to_string()), // Groq GPT-OSS supports reasoning_effort
                "Groq",
            )
        } else {
            // OpenRouter
            let key = config
                .openrouter_api_key
                .as_ref()
                .ok_or("No OpenRouter API key configured")?;
            (
                key.clone(),
                "https://openrouter.ai/api/v1/".to_string(),
                selected_model,
                None, // OpenRouter doesn't use reasoning_effort
                "OpenRouter",
            )
        };

        let url = format!("{}chat/completions", base_url);

        // Load memories for injection into system prompt
        let memory_context = crate::memories::get_memories_for_prompt(app_handle)
            .ok()
            .filter(|s| !s.is_empty());

        let system_prompt_content = if config.incognito_mode.unwrap_or(false) {
            crate::prompts::get_jailbreak_prompt(&model)
        } else if is_research_mode {
            crate::prompts::get_research_system_prompt()
        } else {
            config.system_prompt.clone().unwrap_or_else(|| {
                crate::prompts::get_default_system_prompt(memory_context.as_deref(), rag_context)
            })
        };

        let mut messages_with_system = vec![ChatMessage {
            role: "system".to_string(),
            content: Some(system_prompt_content),
            reasoning: None,
            tool_calls: None,
            tool_call_id: None,
            images: None,
        }];
        messages_with_system.extend(history.clone());

        let api_messages: Vec<ApiChatMessage> = messages_with_system
            .iter()
            .map(|msg| ApiChatMessage {
                role: msg.role.clone(),
                content: msg.content.clone(),
                tool_calls: msg.tool_calls.clone(),
                tool_call_id: msg.tool_call_id.clone(),
            })
            .collect();

        let make_request = |tools_opt: Option<Vec<ToolDefinition>>| {
            let model = model.clone();
            let messages = api_messages.clone();
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
                    include_reasoning: if is_cerebras || is_groq { None } else { Some(true) },
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

        let is_olmo_think = model.contains("olmo-3.1-32b-think");
        let current_tools = if enable_tools && !is_olmo_think {
            Some(
                crate::tools::get_all_tools()
                    .iter()
                    .map(|t| ToolDefinition {
                        tool_type: t.tool_type.clone(),
                        function: FunctionDefinition {
                            name: t.function.name.clone(),
                            description: t.function.description.clone(),
                            parameters: t.function.parameters.clone(),
                            strict: t.function.strict, // Required by Cerebras
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
            println!("[{}] Got 404 with tools, retrying without tools...", provider_name);
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
                    app_handle.emit("agent-fallback", fallback_event.to_string()).ok();

                    // Rebuild request for OpenRouter
                    let openrouter_url = "https://openrouter.ai/api/v1/chat/completions";
                    // Use GPT-OSS-120b on OpenRouter as fallback
                    let fallback_model = "openai/gpt-oss-120b:free".to_string();

                    let fallback_body = ChatCompletionRequest {
                        model: fallback_model,
                        messages: api_messages.clone(),
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

                    response = self.http_client
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
                        app_handle.emit("agent-error", format!("OpenRouter fallback error: {}", fallback_error)).ok();
                        return Err(format!("OpenRouter fallback error: {}", fallback_error));
                    }
                    // Continue with fallback response
                } else {
                    // No OpenRouter key available, show original error
                    app_handle.emit("agent-error", format!("{} error: {}", provider_name, error_text)).ok();
                    return Err(format!("{} error: {}", provider_name, error_text));
                }
            } else {
                // Not a quota error or already on OpenRouter, show original error
                app_handle.emit("agent-error", format!("{} error: {}", provider_name, error_text)).ok();
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

        if !full_content.is_empty() || !tool_calls_buffer.is_empty() {
            history.push(ChatMessage {
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
                images: None,
            });

            if !tool_calls_buffer.is_empty() {
                for tool_call in &tool_calls_buffer {
                    let function_name = &tool_call.function.name;
                    let arguments = &tool_call.function.arguments;
                    let args: Value = serde_json::from_str(arguments).unwrap_or(json!({}));

                    let tool_call_event = json!({
                        "name": function_name,
                        "args": args
                    });
                    app_handle
                        .emit("agent-tool-call", tool_call_event.to_string())
                        .ok();

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

                    history.push(ChatMessage {
                        role: "tool".to_string(),
                        content: Some(tool_result),
                        reasoning: None,
                        tool_calls: None,
                        tool_call_id: Some(tool_call.id.clone()),
                        images: None,
                    });
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
