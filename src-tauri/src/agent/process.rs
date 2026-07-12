//! `Agent::process_message` — top-level orchestration for one user turn.
//!
//! Responsibilities, in order:
//!   1. Image preprocessing (3-way routing: Gemini Files API upload,
//!      OpenRouter native multimodal, or non-vision Vision-LLM fallback).
//!   2. Push the augmented user message onto history + emit `user-message`
//!      (suppressed for cron runs).
//!   3. Generate a user embedding (text or multimodal) and assemble RAG +
//!      Honcho-style peer-card context, unless incognito.
//!   4. Trigger compaction if `enable_compaction` and the token budget is
//!      above `compaction_threshold`.
//!   5. Decide research mode (config flag OR Gemini intent classifier).
//!   6. Run up to `max_turns` provider turns, with empty-response retry
//!      handling and pending-hint injection.
//!   7. Log user + assistant interactions for future RAG (skip in incognito).
//!   8. Persist history and (when ≥2 user + ≥2 assistant messages with a
//!      changed hash) spawn an auto-archive task.

use tauri::{AppHandle, Emitter};

use super::hash::calculate_history_hash;
use super::types::{ChatMessage, ImageAttachment, RetryReason};
use super::{Agent, TurnContext};

impl<R: tauri::Runtime> Agent<R> {
    pub async fn process_message(
        &self,
        app_handle: &AppHandle<R>,
        message: String,
        images_base64: Option<Vec<String>>,
        images_mime_types: Option<Vec<String>>,
        config: &crate::config::AppConfig,
        is_cron: bool,
    ) -> Result<(), String> {
        println!("process_message called. Message len: {}", message.len());

        let mut history = self.history.lock().await;

        // Determine model type using the centralized registry
        let selected_model = config
            .selected_model
            .clone()
            .unwrap_or("gemini-3.1-flash-lite-preview".to_string());
        let is_gemini = crate::models::is_gemini_model(&selected_model);
        let has_native_vision = crate::models::model_supports_vision(&selected_model);

        // Process images with 3-way routing:
        //   1. Gemini: Upload via Files API (native multimodal with file URIs)
        //   2. Vision-capable OpenRouter: Store base64, pass natively via to_multimodal_messages()
        //   3. Non-vision model: Vision LLM fallback for text description + store base64 for history
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
                        // Gemini: Upload to Files API for native multimodal
                        let upload_config = crate::gemini_files::GeminiFilesUploadConfig {
                            upload_url: crate::endpoints::gemini_files_upload(),
                            auth_token: config
                                .gemini_api_key
                                .as_ref()
                                .ok_or("No Gemini API key")?
                                .clone(),
                        };
                        match crate::gemini_files::upload_image_to_gemini_files_api(
                            &self.http_client,
                            img_data,
                            mime_type,
                            &upload_config,
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
                    } else if has_native_vision {
                        // Vision-capable OpenRouter model: images will be sent natively
                        // via to_multimodal_messages() as inline data URIs
                        log::info!(
                            "[Agent] Model {} supports vision — sending image natively",
                            selected_model
                        );
                        None
                    } else {
                        // Non-vision model: use Vision LLM to produce text description
                        let vision_config = crate::integrations::vision_llm::VisionLlmConfig {
                            openrouter_auth_token: config.openrouter_api_key.clone(),
                            groq_auth_token: config.groq_api_key.clone(),
                            endpoints: crate::integrations::vision_llm::VisionLlmEndpoints {
                                openrouter_chat_url: crate::endpoints::openrouter_chat(),
                                groq_chat_url: crate::endpoints::groq_chat(),
                            },
                        };
                        match crate::integrations::vision_llm::process_image_with_context(
                            &self.http_client,
                            img_data,
                            mime_type,
                            &message,
                            &vision_config,
                        )
                        .await
                        {
                            Ok(contextual_response) => {
                                log::info!(
                                    "[Agent] Vision LLM contextual response: {} chars",
                                    contextual_response.len()
                                );
                                image_descriptions.push(contextual_response);
                            }
                            Err(e) => {
                                log::warn!(
                                    "[Agent] Vision LLM contextual processing failed: {}",
                                    e
                                );
                                image_descriptions
                                    .push("[Image attached but could not be analyzed]".to_string());
                            }
                        }
                        None
                    };

                    // Always store image data on the attachment for history fidelity
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

        // For non-vision models, prepend contextual image analysis to the message
        let augmented_message =
            if !is_gemini && !has_native_vision && !image_descriptions.is_empty() {
                let analysis = image_descriptions.join("\n\n");
                format!(
                    "[Visual Analysis]\n{}\n\n[User Message]\n{}",
                    analysis, message
                )
            } else {
                message.clone()
            };

        let msg = ChatMessage {
            role: "user".to_string(),
            content: Some(augmented_message),
            reasoning: None,
            tool_calls: None,
            tool_call_id: None,
            images: uploaded_images,
            is_cron: if is_cron { Some(true) } else { None },
        };
        history.push(msg.clone());
        self.insert_single_message_to_db(app_handle, &msg).await;

        if !is_cron {
            let json_msg = serde_json::to_string(&msg).unwrap_or_default();
            app_handle.emit("user-message", json_msg).ok();
        }

        // Incognito mode: skip all RAG/memory retrieval and storage
        let incognito = config.incognito_mode.unwrap_or(false);

        // RAG: Generate embedding (multimodal when images present) and retrieve relevant interactions using hybrid search (BM25 + Dense + RRF)
        // Skip in incognito mode to avoid using previous context
        let user_embedding = if !incognito {
            if let Some(api_key) = &config.gemini_api_key {
                let embedding_config = crate::gemini_embedding::GeminiEmbeddingConfig {
                    endpoint_url: crate::endpoints::gemini_embedding(),
                    auth_token: api_key.clone(),
                    output_dimensionality: Some(768),
                };
                if let (Some(bases), Some(mimes)) =
                    (images_base64.as_ref(), images_mime_types.as_ref())
                {
                    if !bases.is_empty() {
                        crate::gemini_embedding::generate_multimodal_embedding(
                            &self.http_client,
                            &message,
                            bases,
                            mimes,
                            &embedding_config,
                        )
                        .await
                        .ok()
                    } else {
                        crate::gemini_embedding::generate_embedding(
                            &self.http_client,
                            &message,
                            &embedding_config,
                        )
                        .await
                        .ok()
                    }
                } else {
                    crate::gemini_embedding::generate_embedding(
                        &self.http_client,
                        &message,
                        &embedding_config,
                    )
                    .await
                    .ok()
                }
            } else {
                None
            }
        } else {
            None
        };

        // RAG + Honcho-style context assembly (replaces inline interaction/topic/insight search)
        let (rag_context_str, peer_card_ctx, peer_rep_ctx) = if let Some(emb) = &user_embedding {
            let session_ctx = crate::context::build_session_context(
                app_handle,
                &self.http_client,
                config,
                &message,
                emb,
            )
            .await;
            (
                session_ctx.rag_context_str(),
                session_ctx.peer_card_str().map(String::from),
                session_ctx.peer_representation_str().map(String::from),
            )
        } else {
            (None, None, None)
        };

        // Phase 3.1 — Surface any in-progress action sketches at the top of
        // the RAG slot so the agent can resume a multi-step refactor even
        // after a compaction has scrubbed the original `action_plan` call
        // from chat history. Best-effort; if the store is unavailable or
        // there are no open sketches, we leave rag_context_str unchanged.
        let rag_context_str = match (
            crate::memories::get_vector_store(app_handle)
                .ok()
                .and_then(|s| crate::actions::pending_sketch_summary_text(&s)),
            rag_context_str,
        ) {
            (Some(sketches), Some(rag)) => Some(format!("{}\n{}", sketches, rag)),
            (Some(sketches), None) => Some(sketches),
            (None, rag) => rag,
        };

        // ====================================================================
        // Compaction: Check if we're approaching context window limits
        // ====================================================================
        let compaction_enabled = config.enable_compaction.unwrap_or(true) && !incognito;
        log::info!(
            "[Agent] Compaction check: enabled={}, incognito={}, history_len={}",
            compaction_enabled,
            incognito,
            history.len()
        );

        if compaction_enabled {
            let selected_model = config
                .selected_model
                .clone()
                .unwrap_or("gemini-3.1-flash-lite-preview".to_string());
            let threshold = config.compaction_threshold;

            let current_tokens = crate::compaction::estimate_history_tokens(&history);
            let context_size = crate::compaction::get_context_size(&selected_model);
            let threshold_pct = threshold.unwrap_or(crate::compaction::DEFAULT_THRESHOLD);
            let threshold_tokens = (context_size as f32 * threshold_pct) as usize;

            log::info!(
                "[Agent] Compaction: model={}, tokens={}, context={}, threshold={}% ({} tokens)",
                selected_model,
                current_tokens,
                context_size,
                (threshold_pct * 100.0) as u32,
                threshold_tokens
            );

            if crate::compaction::should_compact(&history, &selected_model, Some(threshold_pct)) {
                log::info!(
                    "[Agent] Context approaching {}% of window - triggering compaction",
                    (threshold_pct * 100.0) as u32
                );

                // Phase 1.1 — pre-compaction lifecycle hook. Lets registered
                // hooks (e.g. Phase 3 action-frontier preservation) capture
                // current state before pre_compaction_flush rewrites history.
                let session_id_for_hook = self.session_id.lock().await.clone();
                self.hooks
                    .dispatch_pre_compact(&session_id_for_hook, current_tokens);

                // Emit compaction event for UI feedback
                let compaction_event = serde_json::json!({
                    "status": "starting",
                    "history_len": history.len()
                });
                app_handle
                    .emit("agent-compaction", compaction_event.to_string())
                    .ok();

                // Pre-compaction flush: extract important facts before summarization
                match crate::compaction::pre_compaction_flush(
                    app_handle,
                    &self.http_client,
                    config,
                    &history,
                )
                .await
                {
                    Ok(flush_result) => {
                        if flush_result.extracted {
                            log::info!(
                                "[Agent] Pre-compaction flush: {} facts saved to daily log",
                                flush_result.fact_count
                            );
                        }
                    }
                    Err(e) => {
                        log::warn!("[Agent] Pre-compaction flush failed: {}", e);
                        // Continue with compaction even if flush fails
                    }
                }

                // Compact history
                match crate::compaction::compact_history(
                    app_handle,
                    &self.http_client,
                    config,
                    &mut history,
                )
                .await
                {
                    Ok(result) => {
                        log::info!(
                            "[Agent] Compacted {} turns, preserved {}, saved ~{} tokens",
                            result.compacted_turns,
                            result.preserved_turns,
                            result.tokens_saved
                        );

                        // Emit completion event
                        let complete_event = serde_json::json!({
                            "status": "complete",
                            "compacted_turns": result.compacted_turns,
                            "preserved_turns": result.preserved_turns,
                            "tokens_saved": result.tokens_saved
                        });
                        app_handle
                            .emit("agent-compaction", complete_event.to_string())
                            .ok();
                    }
                    Err(e) => {
                        log::error!("[Agent] Compaction failed: {}", e);
                        // Continue processing without compaction
                    }
                }
            } else {
                log::info!(
                    "[Agent] Compaction not needed: {} < {} tokens",
                    current_tokens,
                    threshold_tokens
                );
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
                .unwrap_or("gemini-3.1-flash-lite-preview".to_string());

            // Detect provider using centralized model registry
            let is_gemini = crate::models::is_gemini_model(&selected_model);

            // Inject retry hint if pending (from previous failed attempt)
            if let Some(hint) = pending_retry_hint.take() {
                let msg = ChatMessage {
                    role: "user".to_string(),
                    content: Some(hint),
                    reasoning: None,
                    tool_calls: None,
                    tool_call_id: None,
                    is_cron: None,
                    images: None,
                };
                history.push(msg.clone());
                self.insert_single_message_to_db(app_handle, &msg).await;
            }

            let turn_ctx = TurnContext {
                rag_context: rag_context_str.as_deref(),
                peer_card: peer_card_ctx.as_deref(),
                peer_representation: peer_rep_ctx.as_deref(),
                is_research_mode,
                is_cron,
            };
            let continue_turn = if is_gemini {
                self.process_gemini_turn(app_handle, config, &mut history, stream_id, &turn_ctx)
                    .await?
            } else {
                // OpenRouter and Groq use OpenAI-compatible API
                self.process_openrouter_turn(app_handle, config, &mut history, stream_id, &turn_ctx)
                    .await?
            };

            // Check if we need to retry (empty response with reasoning)
            if !continue_turn && retry_on_empty && retry_count < max_retries {
                if let Some(last_msg) = history.last() {
                    let has_reasoning = last_msg
                        .reasoning
                        .as_ref()
                        .map(|r| !r.is_empty())
                        .unwrap_or(false);
                    let has_content = last_msg
                        .content
                        .as_ref()
                        .map(|c| !c.trim().is_empty())
                        .unwrap_or(false);
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

            // Notify frontend when retries are exhausted
            if !continue_turn && retry_count >= max_retries && retry_count > 0 {
                let exhausted_event = serde_json::json!({
                    "reason": "empty_response",
                    "attempts": retry_count,
                    "max": max_retries
                });
                app_handle
                    .emit("agent-retry-exhausted", exhausted_event.to_string())
                    .ok();
            }

            if !continue_turn {
                break;
            }
        }

        // Log interactions for future RAG (skip in incognito mode - use variable defined earlier)
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
                        let embedding_config = crate::gemini_embedding::GeminiEmbeddingConfig {
                            endpoint_url: crate::endpoints::gemini_embedding(),
                            auth_token: api_key.clone(),
                            output_dimensionality: Some(768),
                        };
                        crate::gemini_embedding::generate_embedding(
                            &self.http_client,
                            content,
                            &embedding_config,
                        )
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
        }

        // Persist history to disk after each message exchange (always, regardless of incognito RAG)
        drop(history); // Release lock before persist
        self.persist_history().await;

        // ── Auto-archive: generate session title + summary after 2 user + 2 agent messages ──
        // Fires once per content change when the session crosses the 2+2 threshold.
        // Uses last_archived_hash so it won't re-fire on every subsequent message if
        // the content matches what was previously archived (e.g. no new messages since last archive).
        if !incognito {
            let history_snapshot = self.history.lock().await.clone();
            let user_msgs = history_snapshot.iter().filter(|m| m.role == "user").count();
            let asst_msgs = history_snapshot
                .iter()
                .filter(|m| m.role == "assistant" || m.role == "model")
                .count();

            if user_msgs >= 2 && asst_msgs >= 2 {
                let current_hash = calculate_history_hash(&history_snapshot);
                let last_hash = *self.last_archived_hash.lock().await;

                if current_hash != last_hash {
                    // Update the hash eagerly to prevent concurrent duplicate archives
                    *self.last_archived_hash.lock().await = current_hash;

                    let session_id_now = self.session_id.lock().await.clone();
                    let app_handle_clone = self.app_handle.clone();
                    let http_client_clone = self.http_client.clone();
                    tauri::async_runtime::spawn(async move {
                        if let Ok(config) = crate::config::load_config(&app_handle_clone) {
                            if let Err(e) = crate::sessions::archive_session_transcript(
                                &app_handle_clone,
                                &http_client_clone,
                                &config,
                                &session_id_now,
                                history_snapshot,
                            )
                            .await
                            {
                                log::warn!("[Agent] Auto-archive failed: {}", e);
                            } else {
                                log::info!("[Agent] Auto-archived session after 2+2 messages");
                            }
                        }
                    });
                }
            }
        }

        Ok(())
    }
}
