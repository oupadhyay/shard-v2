//! Tool execution and dispatch for the agent.
//!
//! [`Agent::execute_tool`] is the cache-aware entry point: it consults the
//! per-tool TTL cache from [`crate::cache`] before invoking
//! [`Agent::execute_tool_uncached`], and stores successful results back. Any
//! result whose body starts with `"Error"` is intentionally NOT cached so
//! transient failures can be retried.
//!
//! [`Agent::execute_tool_uncached`] is a single dispatch `match` over the
//! function name, calling the appropriate integration helper or in-process
//! routine. Tools that mutate persistent memory (`save_memory`,
//! `update_topic_summary`, `refresh_memories`) are short-circuited in
//! incognito mode.

use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager};

use crate::integrations::{
    arxiv::{perform_arxiv_lookup, read_arxiv_paper},
    finance::perform_finance_lookup,
    weather::perform_weather_lookup,
    web_search::perform_web_search,
    wikipedia::perform_wikipedia_lookup,
};

use super::Agent;

impl<R: tauri::Runtime> Agent<R> {
    pub(crate) async fn execute_tool(
        &self,
        app_handle: &AppHandle<R>,
        function_name: &str,
        args: &Value,
        config: &crate::config::AppConfig,
    ) -> String {
        // Phase 1.1 — pre-tool-use lifecycle hook. Hooks can short-circuit with
        // a synthetic result (`Replace`) or refuse the call (`Abort`).
        let invocation = super::hooks::ToolInvocation {
            name: function_name,
            args,
            call_id: None,
        };
        match self.hooks.dispatch_pre_tool(&invocation) {
            super::hooks::HookOutcome::Continue => {}
            super::hooks::HookOutcome::Replace(replacement) => {
                let outcome = super::hooks::ToolOutcome {
                    name: function_name,
                    args,
                    call_id: None,
                    result: &replacement,
                    is_error: false,
                };
                self.hooks.dispatch_post_tool(&outcome);
                return replacement;
            }
            super::hooks::HookOutcome::Abort(reason) => {
                let err = format!("Error: {}", reason);
                let outcome = super::hooks::ToolOutcome {
                    name: function_name,
                    args,
                    call_id: None,
                    result: &err,
                    is_error: true,
                };
                self.hooks.dispatch_post_tool(&outcome);
                return err;
            }
        }

        // Check cache first for cacheable tools
        if let Some(cached) = crate::cache::get_cached_result(app_handle, function_name, args) {
            log::info!(
                "[Tool] Cache HIT for {} - returning cached result",
                function_name
            );
            let outcome = super::hooks::ToolOutcome {
                name: function_name,
                args,
                call_id: None,
                result: &cached,
                is_error: false,
            };
            self.hooks.dispatch_post_tool(&outcome);
            return cached;
        }

        let result = self
            .execute_tool_uncached(app_handle, function_name, args, config)
            .await;

        // Cache the result if eligible (never cache errors)
        if !result.starts_with("Error") {
            crate::cache::cache_result(app_handle, function_name, args, &result);
        }

        let outcome = super::hooks::ToolOutcome {
            name: function_name,
            args,
            call_id: None,
            result: &result,
            is_error: result.starts_with("Error"),
        };
        self.hooks.dispatch_post_tool(&outcome);

        result
    }

    /// The actual tool execution logic (separated for caching wrapper)
    async fn execute_tool_uncached(
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
                    Ok(json_str) => json_str,
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
                        serde_json::to_string(&results)
                            .unwrap_or_else(|_| "Failed to serialize search results to JSON".to_string())
                    }
                    Err(e) => format!("Error: {}", e),
                }
            }
            "open_url" => {
                let url = args["url"].as_str().unwrap_or_default();
                match crate::integrations::browser::read_url(&self.http_client, url).await {
                    Ok(markdown) => {
                        format!("Read URL Results for {}:\n\n{}", url, markdown)
                    }
                    Err(e) => format!("Error reading URL: {}", e),
                }
            }
            "youtube_transcript" => {
                let video = args["video"].as_str().unwrap_or_default();
                let video_id = match crate::integrations::youtube::extract_video_id(video) {
                    Some(id) => id,
                    None => return format!("Error: Could not extract a YouTube video ID from '{}'", video),
                };
                match crate::integrations::youtube::fetch_transcript(&self.http_client, &video_id).await {
                    Ok(result) => {
                        let formatted = crate::integrations::youtube::format_transcript(
                            &result.segments,
                            result.title.as_deref(),
                            result.channel.as_deref(),
                        );
                        let title_label = result.title.as_deref().unwrap_or(&video_id);
                        let video_link = format!("https://youtu.be/{}", video_id);
                        // Truncate very long transcripts to avoid blowing up context,
                        // but generate a chunked LLM summary of the full content so nothing is lost.
                        let char_count = formatted.chars().count();
                        if char_count > 30_000 {
                            // Find byte offset of the 30,000th character
                            let truncate_at = formatted
                                .char_indices()
                                .nth(30_000)
                                .map(|(i, _)| i)
                                .unwrap_or(formatted.len());

                            // Summarize the full transcript via background LLM (chunked for long transcripts)
                            let summary = self.summarize_long_transcript(config, &formatted, title_label).await;

                            let summary_section = match &summary {
                                Ok(s) => format!(
                                    "\n\n--- LLM Summary of Full Video ---\n\n{}\n\n--- End Summary ---",
                                    s
                                ),
                                Err(e) => {
                                    log::warn!("[YouTube] Failed to summarize transcript: {}", e);
                                    String::new()
                                }
                            };

                            format!(
                                "YouTube Transcript — {} ({})\n{} segments, truncated\n\n{}...\n\n[Transcript truncated at ~30,000 chars. Total length: {} chars]{}",
                                title_label,
                                video_link,
                                result.segments.len(),
                                &formatted[..truncate_at],
                                char_count,
                                summary_section,
                            )
                        } else {
                            format!(
                                "YouTube Transcript — {} ({})\n{} segments\n\n{}",
                                title_label,
                                video_link,
                                result.segments.len(),
                                formatted
                            )
                        }
                    }
                    Err(e) => format!("Error fetching transcript: {}", e),
                }
            }
            "save_memory" => {
                // Block in incognito mode
                if config.incognito_mode.unwrap_or(false) {
                    return "Skipped: Memory saving is disabled in incognito mode.".to_string();
                }
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
                // Block in incognito mode
                if config.incognito_mode.unwrap_or(false) {
                    return "Skipped: Topic updates are disabled in incognito mode.".to_string();
                }
                let topic = args["topic"].as_str().unwrap_or_default();
                let content = args["content"].as_str().unwrap_or_default();
                match crate::memories::update_topic_summary(app_handle, topic, content) {
                    Ok(_) => format!(
                        "Topic summary updated: {}. Note: Run `refresh_memories` to rebuild the search index for this change to appear in retrieval.",
                        topic
                    ),
                    Err(e) => format!("Failed to update topic summary: {}", e),
                }
            }
            "read_topic_summary" => {
                // Allow reading in incognito mode (no persistence)
                let topic = args["topic"].as_str().unwrap_or_default();
                match crate::memories::read_topic_summary(app_handle, topic) {
                    Ok(content) => content,
                    Err(e) => format!("Failed to read topic summary: {}", e),
                }
            }
            "refresh_memories" => {
                // Block in incognito mode
                if config.incognito_mode.unwrap_or(false) {
                    return "Skipped: Memory refresh is disabled in incognito mode.".to_string();
                }
                match crate::background::run_summary_job_from_agent(app_handle).await {
                    Ok(result) => {
                        let mut msg = format!(
                            "Memory refresh complete: {} topics updated, {} insights created",
                            result.topics_updated.len(),
                            result.insights_created.len()
                        );
                        if !result.topics_updated.is_empty() {
                            msg.push_str(&format!(
                                "\nTopics: {}",
                                result.topics_updated.join(", ")
                            ));
                        }
                        if !result.insights_created.is_empty() {
                            msg.push_str(&format!(
                                "\nInsights: {}",
                                result.insights_created.join(", ")
                            ));
                        }
                        msg
                    }
                    Err(e) => format!("Memory refresh failed: {}", e),
                }
            }
            "memory_search" => {
                let query = args["query"].as_str().unwrap_or_default();
                let max_results = args["max_results"].as_u64().unwrap_or(5) as usize;
                let min_score = args["min_score"].as_f64().unwrap_or(0.3) as f32;
                let time_filter = args["time_filter"].as_str();

                if let Some(tf) = time_filter {
                    if !tf.is_empty() {
                        let handle = app_handle.clone();
                        let query_str = query.to_string();
                        let tf_str = tf.to_string();
                        return tokio::task::spawn_blocking(move || {
                            if let Ok(store) = crate::memories::get_vector_store(&handle) {
                                crate::db::sessions::search_sessions_by_time(
                                    &store,
                                    &query_str,
                                    &tf_str,
                                    max_results,
                                )
                                .unwrap_or_else(|e| format!("Error searching sessions: {}", e))
                            } else {
                                "Error: Failed to open database".to_string()
                            }
                        })
                        .await
                        .unwrap_or_else(|e| e.to_string());
                    }
                }

                if query.is_empty() {
                    return "Error: query parameter is required".to_string();
                }

                // Generate embedding for the query
                let api_key = match config.gemini_api_key.as_ref() {
                    Some(key) => key.clone(),
                    None => return "Error: memory_search requires a Gemini API key for embedding generation".to_string(),
                };

                let embedding = match crate::interactions::generate_embedding(
                    &self.http_client,
                    query,
                    &api_key,
                )
                .await
                {
                    Ok(emb) => emb,
                    Err(e) => return format!("Error generating query embedding: {}", e),
                };

                // Run hybrid search on a blocking thread (SQLite is sync)
                let handle = app_handle.clone();
                let query_text = query.to_string();
                let search_result = match tokio::task::spawn_blocking(move || {
                    crate::memories::search_memory_chunks(
                        &handle,
                        &query_text,
                        &embedding,
                        max_results,
                        min_score,
                    )
                })
                .await
                {
                    Ok(res) => res,
                    Err(e) => return format!("Error: search task panicked: {}", e),
                };

                match search_result {
                    Ok(chunks) => {
                        if chunks.is_empty() {
                            return "No matching memories found.".to_string();
                        }
                        let results: Vec<serde_json::Value> = chunks
                            .iter()
                            .map(|c| {
                                let source_dir = match c.source_type {
                                    crate::memories::SourceType::Topic => "topics",
                                    crate::memories::SourceType::Insight => "insights",
                                    crate::memories::SourceType::Session => "sessions",
                                };
                                serde_json::json!({
                                    "source": format!("{:?}", c.source_type).to_lowercase(),
                                    "path": format!("{}/{}.md", source_dir, c.source_name),
                                    "heading": c.heading,
                                    "start_line": c.start_line,
                                    "end_line": c.end_line,
                                    "snippet": c.text.chars().take(500).collect::<String>(),
                                })
                            })
                            .collect();
                        serde_json::to_string_pretty(&results)
                            .unwrap_or_else(|_| "Error formatting results".to_string())
                    }
                    Err(e) => format!("Memory search failed: {}", e),
                }
            }
            "memory_get" => {
                let session_id = args["session_id"].as_str();
                if let Some(sid) = session_id {
                    let handle = app_handle.clone();
                    let sid_str = sid.to_string();
                    return tokio::task::spawn_blocking(move || {
                        if let Ok(store) = crate::memories::get_vector_store(&handle) {
                            crate::db::sessions::get_session_transcript(&store, &sid_str)
                                .map(|t| format!("Session {} transcript:\n{}", sid_str, t))
                                .unwrap_or_else(|e| format!("Error getting transcript: {}", e))
                        } else {
                            "Error: Failed to open database".to_string()
                        }
                    })
                    .await
                    .unwrap_or_else(|e| e.to_string());
                }

                let path = args["path"].as_str().unwrap_or_default();
                let from = args["from"].as_u64().unwrap_or(1) as usize;
                let lines = args["lines"].as_u64().unwrap_or(50).min(200) as usize;

                if path.is_empty() {
                    return "Error: path parameter is required".to_string();
                }

                match crate::memories::read_memory_file_lines(app_handle, path, from, lines) {
                    Ok(content) => content,
                    Err(e) => format!("Error: {}", e),
                }
            }
            "list_personas" => {
                let personas = crate::personas::list_available_personas();
                if personas.is_empty() {
                    "No dynamic personas are currently available in the workspace.".to_string()
                } else {
                    format!("Available personas:\n{}", personas.join("\n"))
                }
            }
            "load_persona" => {
                let name = args["name"].as_str().unwrap_or_default();
                if let Some(_content) = crate::personas::resolve_persona_content(name) {
                    let session_id = self.session_id.lock().await.clone();
                    if let Ok(store) = crate::memories::get_vector_store(app_handle) {
                        if let Ok(mut active_personas) = crate::db::sessions::get_active_skills(&store, &session_id) {
                            if !active_personas.contains(&name.to_string()) {
                                active_personas.push(name.to_string());
                                let skills_json = serde_json::to_string(&active_personas).unwrap_or_else(|_| "[]".to_string());
                                let _ = crate::db::sessions::update_active_skills(&store, &session_id, &skills_json);
                                format!("Successfully loaded persona '{}'. The instructions will be active for the rest of this session.", name)
                            } else {
                                format!("Persona '{}' is already active.", name)
                            }
                        } else {
                            "Failed to retrieve active session personas.".to_string()
                        }
                    } else {
                        "Failed to access database.".to_string()
                    }
                } else {
                    format!("Persona '{}' not found. Use `list_personas` to see what is available.", name)
                }
            }
            "unload_persona" => {
                let name = args["name"].as_str().unwrap_or_default();
                let session_id = self.session_id.lock().await.clone();
                if let Ok(store) = crate::memories::get_vector_store(app_handle) {
                    if let Ok(mut active_personas) = crate::db::sessions::get_active_skills(&store, &session_id) {
                        if active_personas.contains(&name.to_string()) {
                            active_personas.retain(|s| s != name);
                            let skills_json = serde_json::to_string(&active_personas).unwrap_or_else(|_| "[]".to_string());
                            let _ = crate::db::sessions::update_active_skills(&store, &session_id, &skills_json);
                            format!("Successfully unloaded persona '{}'.", name)
                        } else {
                            format!("Persona '{}' is not currently active.", name)
                        }
                    } else {
                        "Failed to retrieve active session personas.".to_string()
                    }
                } else {
                    "Failed to access database.".to_string()
                }
            }
            "run_python" => {
                let code = args["code"].as_str().unwrap_or_default();
                if code.trim().is_empty() {
                    return "Error: No code provided.".to_string();
                }

                let resource_dir = app_handle
                    .path()
                    .resource_dir()
                    .unwrap_or_default();

                match crate::sandbox::execute_python(code, resource_dir, 30).await {
                    Ok(result) => {
                        let mut output = String::new();
                        if result.timed_out {
                            output.push_str("**Execution timed out (30s limit)**\n\n");
                        }
                        if result.fuel_exhausted {
                            output.push_str("**Execution halted: instruction limit reached**\n\n");
                        }
                        if !result.stdout.is_empty() {
                            output.push_str("**stdout:**\n```\n");
                            if result.stdout.len() > 20_000 {
                                output.push_str(&result.stdout[..20_000]);
                                output.push_str(&format!(
                                    "\n```\n[Truncated at 20,000 chars. Total: {} chars]\n",
                                    result.stdout.len()
                                ));
                            } else {
                                output.push_str(&result.stdout);
                                output.push_str("\n```\n");
                            }
                        }
                        if !result.stderr.is_empty() {
                            output.push_str("**stderr:**\n```\n");
                            if result.stderr.len() > 5_000 {
                                output.push_str(&result.stderr[..5_000]);
                                output.push_str("\n```\n[stderr truncated]\n");
                            } else {
                                output.push_str(&result.stderr);
                                output.push_str("\n```\n");
                            }
                        }
                        if output.is_empty() {
                            output.push_str("Code executed successfully with no output.");
                        }
                        output
                    }
                    Err(e) => format!("Error: {}", e),
                }
            }
            "wake_me_up_in" => {
                let duration_minutes = args["duration_minutes"].as_u64().unwrap_or(0);
                let context = args["context"].as_str().unwrap_or_default().to_string();

                if duration_minutes == 0 || duration_minutes > 1440 {
                    return "Error: duration_minutes must be between 1 and 1440 (24 hours).".to_string();
                }

                if context.trim().is_empty() {
                    return "Error: context must not be empty.".to_string();
                }

                let duration = std::time::Duration::from_secs(duration_minutes * 60);
                let handle = app_handle.clone();
                let session_id = format!("agent:alarm:{}", uuid::Uuid::new_v4());
                let ctx = context.clone();

                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(duration).await;
                    log::info!("[WakeMeUp] Timer fired after {} min for alarm session", duration_minutes);

                    let spec = crate::heartbeat::HeartbeatSpec {
                        schedule: String::new(), // One-shot, not cron-scheduled
                        session: session_id.clone(),
                        persona: None,
                        max_tool_calls: 3,
                        max_runs_per_day: None,
                        prompt: ctx,
                        filename: "dynamic-alarm".to_string(),
                    };

                    match crate::heartbeat::process_heartbeat_turn(&handle, &spec).await {
                        Ok(_) => log::info!("[WakeMeUp] Alarm processed successfully"),
                        Err(e) => log::error!("[WakeMeUp] Alarm failed: {}", e),
                    }
                });

                format!(
                    "Timer set for {} minute(s). Context: '{}'",
                    duration_minutes,
                    if context.len() > 100 {
                        let boundary = context.floor_char_boundary(100);
                        format!("{}...", &context[..boundary])
                    } else {
                        context
                    }
                )
            }
            "read_file" => {
                let path = args["path"].as_str().unwrap_or_default();
                match crate::self_files::read_allowed_file(app_handle, path) {
                    Ok(contents) => {
                        if contents.is_empty() {
                            format!("(file '{}' is empty or does not yet exist)", path)
                        } else {
                            format!("Contents of {}:\n\n```\n{}\n```", path, contents)
                        }
                    }
                    Err(e) => format!("Error: {}", e),
                }
            }
            "edit_file" => {
                let path = args["path"].as_str().unwrap_or_default();
                let old_str = args["old_str"].as_str().unwrap_or("");
                let new_str = args["new_str"].as_str().unwrap_or("");
                let replace_all = args["replace_all"].as_bool().unwrap_or(false);

                match crate::self_files::edit_allowed_file(
                    app_handle, path, old_str, new_str, replace_all,
                ) {
                    Ok(outcome) => {
                        log::info!(
                            "[edit_file] {} ({} replacement{})",
                            outcome.path,
                            outcome.replacements,
                            if outcome.replacements == 1 { "" } else { "s" }
                        );

                        // Structured event for frontend diff viewer / file tree.
                        let _ = app_handle.emit("file-edited", &outcome);

                        format!(
                            "Edited `{}` ({} replacement{}).\n\n```diff\n{}\n```",
                            outcome.path,
                            outcome.replacements,
                            if outcome.replacements == 1 { "" } else { "s" },
                            outcome.unified_diff
                        )
                    }
                    Err(e) => format!("Error: {}", e),
                }
            }
            "rollback_self_edit" => {
                let path = args["path"].as_str().unwrap_or_default();
                let event_id = args["event_id"].as_str().filter(|s| !s.is_empty());
                if let Err(e) = crate::self_files::validate_logical_path(path) {
                    format!("Error: {}", e)
                } else {
                    let store = match crate::memories::get_vector_store(app_handle) {
                        Ok(s) => s,
                        Err(e) => return format!("Error: vector store unavailable: {}", e),
                    };
                    match crate::file_history::rollback_event(&store, path, event_id) {
                        Ok((reverted_id, len)) => {
                            // Emit a `file-edited` event so the diff viewer
                            // adds a tab for the revert (with a sentinel diff
                            // indicating the original edit's id).
                            let _ = app_handle.emit(
                                "file-edited",
                                serde_json::json!({
                                    "path": path,
                                    "abs_path": "",
                                    "before": "",
                                    "after": "",
                                    "unified_diff": format!("(rolled back to event {})", reverted_id),
                                    "replacements": 0_usize,
                                }),
                            );
                            format!(
                                "Rolled back `{}` to event `{}` ({} bytes restored).",
                                path, reverted_id, len
                            )
                        }
                        Err(e) => format!("Error: {}", e),
                    }
                }
            }
            "file_history" => {
                let path = args["path"].as_str().unwrap_or_default();
                let limit = args["limit"]
                    .as_u64()
                    .map(|n| n.clamp(1, 50) as usize)
                    .unwrap_or(10);
                // Validate the path via the same allow-list as read/edit so
                // the agent can't probe arbitrary file logs.
                if let Err(e) = crate::self_files::validate_logical_path(path) {
                    format!("Error: {}", e)
                } else {
                    let store = match crate::memories::get_vector_store(app_handle) {
                        Ok(s) => s,
                        Err(e) => return format!("Error: vector store unavailable: {}", e),
                    };
                    match crate::file_history::summarize(&store, path, limit) {
                        Ok(summary) => summary,
                        Err(e) => format!("Error: {}", e),
                    }
                }
            }
            _ => format!("Unknown tool: {}", function_name),
        }
    }
}
