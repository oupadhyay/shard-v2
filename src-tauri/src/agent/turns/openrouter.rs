//! Streaming turn handler for OpenAI-compatible chat completions
//! (OpenRouter, Groq, Cerebras).
//!
//! Builds a `ChatCompletionRequest` with provider-aware tweaks:
//!   - Groq omits `include_reasoning` (Groq's reasoning is implicit).
//!   - Models containing `olmo-3.1-32b-think` disable tool calling regardless
//!     of config (they emit reasoning-only output).
//!   - Models containing `upstage` strip the `strict` flag from each tool
//!     definition (Upstage rejects unknown JSON Schema fields).
//!   - During cron runs, prior cron messages are filtered from the visible
//!     history and the current cron prompt is wrapped in a
//!     `<system_directive>` block (with nested directive tags HTML-escaped).
//!
//! On a 404 response while tools are enabled, retries once without tools.
//! On a Groq quota error and an available OpenRouter API key, falls back to
//! `crate::endpoints::openrouter_chat()` with `config.fallback_model`.

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

use super::super::adapters::{
    chat_messages_to_provider, provider_tool_call_to_host, provider_tool_definition_from_host,
};
use super::super::openrouter::{
    process_chat_completion_sse_line, send_chat_completion_request, to_multimodal_messages,
    ChatCompletionRequest, OpenAiChatStreamEvent, OpenAiChatStreamState, OpenAiChatTransportConfig,
};
use super::super::types::ChatMessage;
use super::super::{Agent, TurnContext};
use crate::llm_provider::ProviderToolDefinition;

impl<R: tauri::Runtime> Agent<R> {
    pub(crate) async fn process_openrouter_turn(
        &self,
        app_handle: &AppHandle<R>,
        config: &crate::config::AppConfig,
        history: &mut Vec<ChatMessage>,
        stream_id: u64,
        ctx: &TurnContext<'_>,
    ) -> Result<bool, String> {
        let rag_context = ctx.rag_context;
        let peer_card = ctx.peer_card;
        let peer_representation = ctx.peer_representation;
        let is_research_mode = ctx.is_research_mode;
        let selected_model = config
            .selected_model
            .clone()
            .unwrap_or("gemini-3.1-flash-lite-preview".to_string());
        let enable_tools = config.enable_tools.unwrap_or(true);

        // Detect provider from model name and configure accordingly
        let (provider_config, api_key) =
            config.get_model_provider_config(&selected_model, "main chat")?;
        let is_groq = provider_config.provider_name == "Groq";

        let model = provider_config.model_id.clone();
        let reasoning_effort = provider_config.reasoning_effort.clone();
        let provider_name = provider_config.provider_name.clone();
        let url = if provider_name == "Groq" {
            crate::endpoints::groq_chat()
        } else {
            crate::endpoints::openrouter_chat()
        };

        // Load memories for injection into system prompt (skip in incognito mode)
        let incognito_mode = config.incognito_mode.unwrap_or(false);
        let memory_context = if incognito_mode {
            None
        } else {
            crate::memories::get_memories_for_prompt(app_handle)
                .ok()
                .filter(|s| !s.is_empty())
        };

        let available_skills = crate::personas::list_available_personas();
        let available_skills_str = if available_skills.is_empty() {
            None
        } else {
            Some(available_skills.join("\n"))
        };
        let available_skills_opt = available_skills_str.as_deref();

        let session_id = self.session_id.lock().await.clone();
        let mut active_skills_opt: Option<String> = None;
        if let Ok(store) = crate::memories::get_vector_store(app_handle) {
            if let Ok(active_personas) = crate::db::sessions::get_active_skills(&store, &session_id)
            {
                if !active_personas.is_empty() {
                    let mut active_skills_content = String::new();
                    for persona in active_personas {
                        if let Some(content) = crate::personas::resolve_persona_content(&persona) {
                            active_skills_content.push_str(&format!(
                                "--- PERSONA: {} ---\n{}\n\n",
                                persona, content
                            ));
                        }
                    }
                    if !active_skills_content.is_empty() {
                        active_skills_opt = Some(active_skills_content);
                    }
                }
            }
        }

        let system_prompt_content = if incognito_mode {
            crate::prompts::get_default_system_prompt(
                None,
                None,
                None,
                None,
                available_skills_opt,
                active_skills_opt.as_deref(),
            )
        } else if is_research_mode {
            crate::prompts::get_research_system_prompt(
                available_skills_opt,
                active_skills_opt.as_deref(),
            )
        } else {
            config.system_prompt.clone().unwrap_or_else(|| {
                crate::prompts::get_default_system_prompt(
                    memory_context.as_deref(),
                    rag_context,
                    peer_card,
                    peer_representation,
                    available_skills_opt,
                    active_skills_opt.as_deref(),
                )
            })
        };

        let mut messages_with_system = vec![ChatMessage {
            role: "system".to_string(),
            content: Some(system_prompt_content),
            reasoning: None,
            tool_calls: None,
            tool_call_id: None,
            is_cron: None,
            images: None,
        }];
        // 1. Filter out past cron messages from the LLM's context window ONLY during a cron run,
        // so the active cron job focuses on the user's actual conversation. Regular users still see them.
        //
        // Use the explicit cron flag from `TurnContext` rather than deriving it from
        // `history.last().is_cron`. On subsequent turns inside the `process_message`
        // loop the last message is a tool/assistant message (which has `is_cron =
        // None`), so deriving from history would silently disable cron-aware
        // filtering and `<system_directive>` wrapping mid-loop.
        let is_cron = ctx.is_cron;
        let visible_history: Vec<ChatMessage> = if is_cron {
            let len = history.len();
            let mut visible: Vec<ChatMessage> = Vec::with_capacity(len);
            let mut in_past_cron_segment = false;
            for (i, m) in history.iter().enumerate() {
                // Always keep the very last message (the current cron prompt) in the history.
                if i == len.saturating_sub(1) {
                    visible.push(m.clone());
                    break;
                }
                // A message explicitly marked as cron starts a cron segment (typically a cron user prompt).
                if m.is_cron.unwrap_or(false) {
                    in_past_cron_segment = true;
                    continue;
                }
                // A normal user message (without cron flag) ends any prior cron segment.
                if m.role == "user" {
                    in_past_cron_segment = false;
                }
                if !in_past_cron_segment {
                    visible.push(m.clone());
                }
            }
            visible
        } else {
            history.clone()
        };

        messages_with_system.extend(visible_history);

        let last_idx = messages_with_system.len().saturating_sub(1);
        let prepared_messages = messages_with_system
            .into_iter()
            .enumerate()
            .map(|(i, mut msg)| {
                // If this is a cron job, structurally isolate the current cron prompt from the "chat history"
                if is_cron && i == last_idx {
                    if let Some(ref mut c) = msg.content {
                        let sanitized = c
                            .replace("<system_directive>", "&lt;system_directive&gt;")
                            .replace("</system_directive>", "&lt;/system_directive&gt;");
                        *c = format!("<system_directive>\nYou are executing a scheduled background task. Please evaluate the user's task instruction strictly against the conversation history preceding this message. Do not consider this directive itself as part of the chat history or summarize it.\nTask: {}\n</system_directive>", sanitized);
                    }
                }
                msg
            })
            .collect::<Vec<ChatMessage>>();
        let provider_messages = chat_messages_to_provider(&prepared_messages);
        let multimodal_messages = to_multimodal_messages(&provider_messages);

        // Note: multimodal_messages is no longer cloned per request attempt because
        // ChatCompletionRequest is now generic over the messages type, allowing us to borrow.
        // This avoids deep cloning large base64 image data on retry/fallback paths.
        let make_request = |tools_opt: Option<Vec<ProviderToolDefinition>>| {
            let model = model.clone();
            // Borrow messages to avoid cloning
            let messages = multimodal_messages.as_slice();
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
                    include_reasoning: if is_groq { None } else { Some(true) },
                    temperature: None,
                    max_tokens: None,
                    stream: true,
                };

                let transport_config = OpenAiChatTransportConfig {
                    endpoint_url: url,
                    auth_token: api_key,
                };
                send_chat_completion_request(&client, &transport_config, &request_body).await
            }
        };

        let session_id_str = self.session_id.lock().await.clone();
        let active_skills_list = crate::memories::get_vector_store(app_handle)
            .and_then(|store| crate::db::sessions::get_active_skills(&store, &session_id_str))
            .unwrap_or_default();

        let is_olmo_think = model.contains("olmo-3.1-32b-think");
        let is_strict_blacklisted = model.to_lowercase().contains("upstage");

        let current_tools = if enable_tools && !is_olmo_think {
            Some(
                crate::tool_registry::global()
                    .get_definitions(&active_skills_list)
                    .iter()
                    .map(|t| {
                        let mut provider_tool = provider_tool_definition_from_host(t);
                        if is_strict_blacklisted {
                            provider_tool.function.strict = None;
                        }
                        provider_tool
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
            println!(
                "[{}] Got 404 with tools, retrying without tools...",
                provider_name
            );
            response = make_request(None)
                .await
                .map_err(|e| format!("{} network error (retry): {}", provider_name, e))?;
        }

        // Check for token quota errors on Groq and fallback to OpenRouter
        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_default();
            let is_quota_error = error_text.contains("token_quota_exceeded")
                || error_text.contains("too_many_tokens")
                || error_text.contains("rate_limit")
                || error_text.contains("tokens per minute");

            // Only fallback for Groq quota errors, not OpenRouter
            if is_quota_error && is_groq {
                // Check if OpenRouter is available for fallback
                if let Some(openrouter_key) = &config.openrouter_api_key {
                    // Emit fallback notification with original error
                    let fallback_event = serde_json::json!({
                        "title": "API Error: Moving to OpenRouter",
                        "details": format!("{} error: {}", provider_name, error_text)
                    });
                    app_handle
                        .emit("agent-fallback", fallback_event.to_string())
                        .ok();

                    // Rebuild request for OpenRouter
                    let openrouter_url = crate::endpoints::openrouter_chat();
                    // Use configured fallback model or default
                    let fallback_model = config
                        .fallback_model
                        .clone()
                        .unwrap_or_else(|| "openai/gpt-oss-120b:free".to_string());

                    let fallback_body = ChatCompletionRequest {
                        model: fallback_model,
                        messages: multimodal_messages.as_slice(),
                        tools: current_tools.clone(),
                        tool_choice: if current_tools.is_some() {
                            Some("auto".to_string())
                        } else {
                            None
                        },
                        reasoning_effort: None,
                        reasoning: None,
                        include_reasoning: Some(true),
                        temperature: None,
                        max_tokens: None,
                        stream: true,
                    };

                    let fallback_transport_config = OpenAiChatTransportConfig {
                        endpoint_url: openrouter_url,
                        auth_token: openrouter_key.clone(),
                    };

                    response = send_chat_completion_request(
                        &self.http_client,
                        &fallback_transport_config,
                        &fallback_body,
                    )
                    .await
                    .map_err(|e| format!("OpenRouter fallback network error: {}", e))?;

                    // Check if fallback succeeded
                    if !response.status().is_success() {
                        let fallback_error = response.text().await.unwrap_or_default();
                        app_handle
                            .emit(
                                "agent-error",
                                format!("OpenRouter fallback error: {}", fallback_error),
                            )
                            .ok();
                        return Err(format!("OpenRouter fallback error: {}", fallback_error));
                    }
                    // Continue with fallback response
                } else {
                    // No OpenRouter key available, show original error
                    app_handle
                        .emit(
                            "agent-error",
                            format!("{} error: {}", provider_name, error_text),
                        )
                        .ok();
                    return Err(format!("{} error: {}", provider_name, error_text));
                }
            } else {
                // Not a quota error or already on OpenRouter, show original error
                app_handle
                    .emit(
                        "agent-error",
                        format!("{} error: {}", provider_name, error_text),
                    )
                    .ok();
                return Err(format!("{} error: {}", provider_name, error_text));
            }
        }

        let mut stream_state = OpenAiChatStreamState::default();
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
                    if let Some(json_str) = line.strip_prefix("data: ") {
                        if json_str == "[DONE]" {
                            continue;
                        }

                        for event in process_chat_completion_sse_line(line, &mut stream_state) {
                            match event {
                                OpenAiChatStreamEvent::ReasoningDelta(reasoning) => {
                                    app_handle.emit("agent-reasoning-chunk", reasoning).ok();
                                }
                                OpenAiChatStreamEvent::ContentDelta(content) => {
                                    app_handle.emit("agent-response-chunk", content).ok();
                                }
                                OpenAiChatStreamEvent::ToolCallDelta(tool_call) => {
                                    // Emit tool call update for real-time UI mapping
                                    let args_json: serde_json::Value =
                                        serde_json::from_str(&tool_call.function.arguments)
                                            .unwrap_or(serde_json::Value::Object(
                                                serde_json::Map::new(),
                                            ));
                                    let event_payload = serde_json::json!({
                                        "name": tool_call.function.name,
                                        "args": args_json,
                                        "rawArgs": tool_call.function.arguments,
                                        "id": tool_call.id
                                    });
                                    app_handle
                                        .emit("agent-tool-call", event_payload.to_string())
                                        .ok();
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

        let full_content = stream_state.content;
        let full_reasoning = stream_state.reasoning;
        let tool_calls_buffer: Vec<_> = stream_state
            .tool_calls
            .iter()
            .map(provider_tool_call_to_host)
            .collect();

        if !full_content.is_empty() || !tool_calls_buffer.is_empty() || !full_reasoning.is_empty() {
            let msg = ChatMessage {
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
                is_cron: None,
                images: None,
            };
            history.push(msg.clone());
            self.insert_single_message_to_db(app_handle, &msg).await;

            if !tool_calls_buffer.is_empty() {
                for tool_call in &tool_calls_buffer {
                    let function_name = &tool_call.function.name;
                    let arguments = &tool_call.function.arguments;
                    let args: Value = serde_json::from_str(arguments).unwrap_or(json!({}));

                    // Note: agent-tool-call was already emitted during streaming (line ~2259, with id for dedup).
                    // A second emit here duplicated the card in the frontend.

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

                    let msg = ChatMessage {
                        role: "tool".to_string(),
                        content: Some(tool_result),
                        reasoning: None,
                        tool_calls: None,
                        tool_call_id: Some(tool_call.id.clone()),
                        is_cron: None,
                        images: None,
                    };
                    history.push(msg.clone());
                    self.insert_single_message_to_db(app_handle, &msg).await;
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
