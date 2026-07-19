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
        if let Some(result) = crate::tool_registry::try_execute_external_tool(
            &self.http_client,
            config,
            function_name,
            args,
        )
        .await
        {
            return result;
        }

        match function_name {
            "youtube_transcript" => {
                let video = args["video"].as_str().unwrap_or_default();
                match crate::external_tools::fetch_youtube_transcript(&self.http_client, video)
                    .await
                {
                    Ok(transcript) => {
                        let summary = if transcript.char_count() > 30_000 {
                            match self
                                .summarize_long_transcript(
                                    config,
                                    &transcript.formatted,
                                    transcript.title_label(),
                                )
                                .await
                            {
                                Ok(summary) => Some(summary),
                                Err(e) => {
                                    log::warn!("[YouTube] Failed to summarize transcript: {}", e);
                                    None
                                }
                            }
                        } else {
                            None
                        };
                        transcript.render(summary.as_deref())
                    }
                    Err(e) => e,
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
                let embedding_config = crate::gemini_embedding::GeminiEmbeddingConfig {
                    endpoint_url: crate::endpoints::gemini_embedding(),
                    auth_token: api_key,
                    output_dimensionality: Some(768),
                };

                let embedding = match crate::gemini_embedding::generate_embedding(
                    &self.http_client,
                    query,
                    &embedding_config,
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
                        if let Ok(mut active_personas) =
                            crate::db::sessions::get_active_skills(&store, &session_id)
                        {
                            if !active_personas.contains(&name.to_string()) {
                                active_personas.push(name.to_string());
                                let skills_json = serde_json::to_string(&active_personas)
                                    .unwrap_or_else(|_| "[]".to_string());
                                let _ = crate::db::sessions::update_active_skills(
                                    &store,
                                    &session_id,
                                    &skills_json,
                                );
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
                    format!(
                        "Persona '{}' not found. Use `list_personas` to see what is available.",
                        name
                    )
                }
            }
            "unload_persona" => {
                let name = args["name"].as_str().unwrap_or_default();
                let session_id = self.session_id.lock().await.clone();
                if let Ok(store) = crate::memories::get_vector_store(app_handle) {
                    if let Ok(mut active_personas) =
                        crate::db::sessions::get_active_skills(&store, &session_id)
                    {
                        if active_personas.contains(&name.to_string()) {
                            active_personas.retain(|s| s != name);
                            let skills_json = serde_json::to_string(&active_personas)
                                .unwrap_or_else(|_| "[]".to_string());
                            let _ = crate::db::sessions::update_active_skills(
                                &store,
                                &session_id,
                                &skills_json,
                            );
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

                let resource_dir = app_handle.path().resource_dir().unwrap_or_default();

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
                    return "Error: duration_minutes must be between 1 and 1440 (24 hours)."
                        .to_string();
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
                    log::info!(
                        "[WakeMeUp] Timer fired after {} min for alarm session",
                        duration_minutes
                    );

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
                // Return raw file contents (or empty string when the file
                // doesn't exist yet) so the agent can copy substrings
                // verbatim into `edit_file`'s `old_str`. Matches the MCP
                // handler in `crate::mcp::handlers::handle_read_file`.
                let path = args["path"].as_str().unwrap_or_default();
                match crate::self_files::read_allowed_file(app_handle, path) {
                    Ok(contents) => contents,
                    Err(e) => format!("Error: {}", e),
                }
            }
            "edit_file" => {
                let path = args["path"].as_str().unwrap_or_default();
                let old_str = args["old_str"].as_str().unwrap_or("");
                let new_str = args["new_str"].as_str().unwrap_or("");
                let replace_all = args["replace_all"].as_bool().unwrap_or(false);

                match crate::self_files::edit_allowed_file(
                    app_handle,
                    path,
                    old_str,
                    new_str,
                    replace_all,
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
            "action_plan" => {
                let title = args["title"].as_str().unwrap_or_default();
                let steps: Vec<&str> = args["steps"]
                    .as_array()
                    .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
                    .unwrap_or_default();
                if title.is_empty() || steps.is_empty() {
                    "Error: action_plan requires non-empty title and steps".to_string()
                } else {
                    let store = match crate::memories::get_vector_store(app_handle) {
                        Ok(s) => s,
                        Err(e) => return format!("Error: vector store unavailable: {}", e),
                    };
                    let session_id = self.session_id.lock().await.clone();
                    match crate::actions::plan(&store, title, &steps, Some(&session_id)) {
                        Ok(ids) => serde_json::json!({
                            "sketch_id": ids[0],
                            "step_ids": &ids[1..],
                            "count": steps.len(),
                        })
                        .to_string(),
                        Err(e) => format!("Error: {}", e),
                    }
                }
            }
            "action_next" => {
                let store = match crate::memories::get_vector_store(app_handle) {
                    Ok(s) => s,
                    Err(e) => return format!("Error: vector store unavailable: {}", e),
                };
                match crate::actions::frontier(&store) {
                    Ok(Some(a)) => serde_json::json!({
                        "id": a.id,
                        "title": a.title,
                        "priority": a.priority,
                        "parent_id": a.parent_id,
                        "deps": a.deps,
                        "session_id": a.session_id,
                    })
                    .to_string(),
                    Ok(None) => "null".to_string(),
                    Err(e) => format!("Error: {}", e),
                }
            }
            "action_complete" => {
                let id = args["id"].as_str().unwrap_or_default();
                let outcome = args["outcome"].as_str();
                if id.is_empty() {
                    "Error: action_complete requires an id".to_string()
                } else {
                    let store = match crate::memories::get_vector_store(app_handle) {
                        Ok(s) => s,
                        Err(e) => return format!("Error: vector store unavailable: {}", e),
                    };
                    match crate::actions::complete(&store, id, outcome) {
                        Ok(()) => format!("Completed action {}", id),
                        Err(e) => format!("Error: {}", e),
                    }
                }
            }
            "action_block" => {
                let id = args["id"].as_str().unwrap_or_default();
                let reason = args["reason"].as_str().unwrap_or("");
                if id.is_empty() || reason.is_empty() {
                    "Error: action_block requires id and reason".to_string()
                } else {
                    let store = match crate::memories::get_vector_store(app_handle) {
                        Ok(s) => s,
                        Err(e) => return format!("Error: vector store unavailable: {}", e),
                    };
                    match crate::actions::block(&store, id, reason) {
                        Ok(()) => format!("Blocked action {}: {}", id, reason),
                        Err(e) => format!("Error: {}", e),
                    }
                }
            }
            "crystallize_sketch" => {
                let sketch_id = args["sketch_id"].as_str().unwrap_or_default();
                if sketch_id.is_empty() {
                    "Error: crystallize_sketch requires a sketch_id".to_string()
                } else {
                    // Pull the sketch synchronously, then drop the store so
                    // the LLM await doesn't see a non-`Send` connection.
                    let loaded = {
                        let store = match crate::memories::get_vector_store(app_handle) {
                            Ok(s) => s,
                            Err(e) => return format!("Error: vector store unavailable: {}", e),
                        };
                        crate::crystals::load_sketch(&store, sketch_id)
                    };
                    match loaded {
                        Err(e) => format!("Error: {}", e),
                        Ok((parent, children)) => {
                            let existing = crate::personas::list_available_personas();
                            let http_client = reqwest::Client::new();
                            match crate::crystals::crystallize(
                                &http_client,
                                config,
                                &parent,
                                &children,
                                &existing,
                            )
                            .await
                            {
                                Ok(draft) => {
                                    match crate::crystals::write_persona_draft(app_handle, &draft) {
                                        Ok(outcome) => {
                                            if let Ok(store) =
                                                crate::memories::get_vector_store(app_handle)
                                            {
                                                let _ = crate::crystals::mark_crystallized(
                                                    &store, sketch_id,
                                                );
                                            }
                                            format!(
                                            "Crystallised sketch `{}` into persona `{}` ({} bytes written to {}).\n\n```diff\n{}\n```",
                                            sketch_id,
                                            draft.slug,
                                            outcome.after.len(),
                                            outcome.abs_path,
                                            outcome.unified_diff
                                        )
                                        }
                                        Err(e) => format!("Error writing persona draft: {}", e),
                                    }
                                }
                                Err(e) => format!("Error: {}", e),
                            }
                        }
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
