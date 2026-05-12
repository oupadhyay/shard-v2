//! Streaming turn handler for the Gemini *Interactions* API.
//!
//! Builds an `InteractionsRequest` from the current chat history, system
//! prompt (with optional research-mode override and persona injection), and
//! the registered tool list (filtered through [`super::super::schema::normalize_gemini_schema`]
//! to keep Gemini's proto-backed schema validator happy). Streams the
//! response via SSE, accumulates text + reasoning + tool calls, executes any
//! requested tools, and appends the resulting messages onto `history`.

use serde_json::Value;
use tauri::{AppHandle, Emitter};

use super::super::gemini::{
    construct_interactions_input, parse_interactions_sse_line, process_interactions_event,
    AgentEvent,
};
use super::super::schema::normalize_gemini_schema;
use super::super::types::{
    ChatMessage, FunctionCall, InteractionsGenerationConfig, InteractionsRequest,
    InteractionsTool, ToolCall,
};
use super::super::{Agent, TurnContext};

impl<R: tauri::Runtime> Agent<R> {
    pub(crate) async fn process_gemini_turn(
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
        let api_key = config.gemini_api_key.as_ref().ok_or("No Gemini API key")?;
        let enable_tools = config.enable_tools.unwrap_or(true);
        // Interactions API: flat endpoint, model specified in request body
        let url = crate::endpoints::gemini_interactions();

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
        let available_skills_str = if available_skills.is_empty() { None } else { Some(available_skills.join("\n")) };
        let available_skills_opt = available_skills_str.as_deref();

        let session_id = self.session_id.lock().await.clone();
        let mut active_skills_opt: Option<String> = None;
        if let Ok(store) = crate::memories::get_vector_store(app_handle) {
            if let Ok(active_personas) = crate::db::sessions::get_active_skills(&store, &session_id) {
                if !active_personas.is_empty() {
                    let mut active_skills_content = String::new();
                    for persona in active_personas {
                        if let Some(content) = crate::personas::resolve_persona_content(&persona) {
                            active_skills_content.push_str(&format!("--- PERSONA: {} ---\n{}\n\n", persona, content));
                        }
                    }
                    if !active_skills_content.is_empty() {
                        active_skills_opt = Some(active_skills_content);
                    }
                }
            }
        }

        let system_prompt_content = if incognito_mode {
            crate::prompts::get_default_system_prompt(None, None, None, None, available_skills_opt, active_skills_opt.as_deref())
        } else if is_research_mode {
            crate::prompts::get_research_system_prompt(available_skills_opt, active_skills_opt.as_deref())
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

        // Build stateless input from history
        let input = construct_interactions_input(history);

        let session_id_str = self.session_id.lock().await.clone();
        let active_skills_list = crate::memories::get_vector_store(app_handle)
            .and_then(|store| crate::db::sessions::get_active_skills(&store, &session_id_str))
            .unwrap_or_default();

        // Interactions API uses flat tool definitions: { type: "function", name, description, parameters }
        let interactions_tools: Option<Vec<InteractionsTool>> = if enable_tools {
            Some(
                crate::tool_registry::global().get_definitions(&active_skills_list)
                    .iter()
                    .map(|t| {
                        let mut params = t.function.parameters.clone();
                        normalize_gemini_schema(&mut params);
                        InteractionsTool::Function {
                            name: t.function.name.clone(),
                            description: t.function.description.clone(),
                            parameters: params,
                        }
                    })
                    .collect(),
            )
        } else {
            None
        };

        let supports_thinking = selected_model.contains("2.5")
            || selected_model.contains("gemini-3")
            || selected_model.contains("thinking");

        let request_body = InteractionsRequest {
            model: selected_model.to_string(),
            input,
            system_instruction: Some(system_prompt_content),
            tools: interactions_tools,
            generation_config: if supports_thinking {
                Some(InteractionsGenerationConfig {
                    thinking_level: Some("high".to_string()),
                    thinking_summaries: Some("auto".to_string()),
                    temperature: None,
                    max_output_tokens: None,
                })
            } else {
                None
            },
            stream: true,
            store: Some(false), // We manage state locally
        };

        // DEBUG: Output the raw REST JSON to terminal so we can see what's being rejected
        if cfg!(debug_assertions) {
            if let Ok(json) = serde_json::to_string_pretty(&request_body) {
                println!("--- GEMINI REQUEST PAYLOAD ---\n{}\n------------------------------", json);
            }
        }

        // Streaming via SSE: append ?alt=sse
        let response = self
            .http_client
            .post(format!("{}?alt=sse", url))
            .header("x-goog-api-key", api_key)
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send()
            .await
            .map_err(|e| format!("API network error: {}", e))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            let msg = if error_text.is_empty() {
                format!("Gemini API Error (HTTP {})", status)
            } else {
                format!("Gemini API Error (HTTP {}): {}", status, error_text)
            };
            log::warn!("[Gemini] {}", msg);
            app_handle.emit("agent-error", &msg).ok();
            return Err(msg);
        }

        use futures_util::StreamExt;
        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        let mut full_text = String::new();
        let mut full_reasoning = String::new();
        let mut tool_calls: Vec<(String, String, Value, Option<String>)> = Vec::new(); // (id, name, arguments, signature)
        let mut current_signature: Option<String> = None;

        while let Some(item) = stream.next().await {
            if stream_id == crate::CANCELLED_STREAM_ID.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }

            let chunk = item.map_err(|e| format!("Stream error: {}", e))?;
            let chunk_str = String::from_utf8_lossy(&chunk);
            buffer.push_str(&chunk_str);

            // Process complete SSE lines
            while let Some(newline_pos) = buffer.find('\n') {
                let line = buffer[..newline_pos].trim_end_matches('\r').to_string();
                buffer.drain(..newline_pos + 1);

                if line.is_empty() {
                    continue;
                }

                if let Some(event) = parse_interactions_sse_line(&line) {
                    let events = process_interactions_event(
                        &event,
                        &mut full_text,
                        &mut full_reasoning,
                    );
                    for agent_event in events {
                        match agent_event {
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
                            AgentEvent::InteractionToolCall { id, name, arguments, signature } => {
                                if let Some(sig) = signature {
                                    // Try to attach signature to an existing tool call by id
                                    if let Some(entry) = tool_calls.iter_mut().find(|e| e.0 == id) {
                                        entry.3 = Some(sig);
                                    } else {
                                        current_signature = Some(sig);
                                    }
                                } else {
                                    // Check if a placeholder already exists for this id (signature arrived first)
                                    if let Some(entry) = tool_calls.iter_mut().find(|e| e.0 == id) {
                                        entry.1 = name.clone();
                                        entry.2 = arguments.clone();
                                    } else {
                                        tool_calls.push((id.clone(), name.clone(), arguments.clone(), current_signature.take()));
                                    }
                                    let tool_call_event = serde_json::json!({
                                        "name": name,
                                        "args": arguments,
                                        "rawArgs": serde_json::to_string(&arguments).unwrap_or_default(),
                                        "id": id,
                                    });
                                    app_handle
                                        .emit("agent-tool-call", tool_call_event.to_string())
                                        .ok();
                                }
                            }
                            AgentEvent::ToolCall(fc) => {
                                // Legacy path (should not fire for Interactions API)
                                let tool_call_event = serde_json::json!({
                                    "name": fc.function_call.name,
                                    "args": fc.function_call.args,
                                    "rawArgs": serde_json::to_string(&fc.function_call.args).unwrap_or_default(),
                                    "id": format!("call_{}", fc.function_call.name)
                                });
                                app_handle
                                    .emit("agent-tool-call", tool_call_event.to_string())
                                    .ok();
                            }
                        }
                    }
                }
            }
        }

        if !tool_calls.is_empty() {
            let msg = ChatMessage {
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
                        .map(|(id, name, args, signature)| ToolCall {
                            id: id.clone(),
                            tool_type: "function".to_string(),
                            function: FunctionCall {
                                name: name.clone(),
                                arguments: serde_json::to_string(args)
                                    .unwrap_or_default(),
                            },
                            thought_signature: signature.clone(),
                        })
                        .collect(),
                ),
                tool_call_id: None,
                is_cron: None,
                images: None,
            };
            history.push(msg.clone());
            self.insert_single_message_to_db(app_handle, &msg).await;

            for (id, name, args, _) in tool_calls.iter() {
                let tool_result = self
                    .execute_tool(app_handle, name, args, config)
                    .await;

                let result_payload = serde_json::json!({
                    "name": name,
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
                    tool_call_id: Some(id.clone()),
                    is_cron: None,
                    images: None,
                };
                history.push(msg.clone());
                self.insert_single_message_to_db(app_handle, &msg).await;
            }
            Ok(true) // Continue loop so model can respond to tool results
        } else {
            let msg = ChatMessage {
                role: "assistant".to_string(),
                content: if full_text.is_empty() {
                    None
                } else {
                    Some(full_text)
                },
                reasoning: if full_reasoning.is_empty() {
                    None
                } else {
                    Some(full_reasoning.trim_end().to_string())
                },
                tool_calls: None,
                tool_call_id: None,
                is_cron: None,
                images: None,
            };
            history.push(msg.clone());
            self.insert_single_message_to_db(app_handle, &msg).await;
            Ok(false) // No tool calls = final response, stop the loop
        }
    }
}
