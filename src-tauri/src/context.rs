//! Budgeted Context Assembly (Honcho-inspired)
//!
//! Replaces the inline RAG assembly in `agent/mod.rs` with a structured
//! context builder that combines:
//! - Existing: interaction search + topic/insight retrieval
//! - New: Honcho-style observation representation + peer card
//!
//! The output is a single `SessionContext` struct ready for prompt injection.

use tauri::{AppHandle, Runtime};

// ============================================================================
// Types
// ============================================================================

/// Assembled context ready for injection into the system prompt.
/// Replaces the ad-hoc `rag_context_str` in `process_message()`.
pub struct SessionContext {
    /// Formatted string of relevant past interactions (existing RAG).
    pub interactions: Option<String>,
    /// Matched topic or insight content (existing RAG).
    pub topic_or_insight: Option<String>,
    /// Honcho-style observation-based user representation.
    pub peer_representation: Option<String>,
    /// Honcho-style peer card (biographical facts).
    pub peer_card: Option<String>,
}

impl SessionContext {
    /// Get just the peer card string (for dedicated prompt slot).
    pub fn peer_card_str(&self) -> Option<&str> {
        self.peer_card.as_deref().filter(|s| !s.is_empty())
    }

    /// Get just the peer representation string (for dedicated prompt slot).
    pub fn peer_representation_str(&self) -> Option<&str> {
        self.peer_representation
            .as_deref()
            .filter(|s| !s.is_empty())
    }

    /// Get just the RAG context (interactions + topics, without peer data).
    pub fn rag_context_str(&self) -> Option<String> {
        let mut parts = Vec::new();
        if let Some(ref interactions) = self.interactions {
            if !interactions.is_empty() {
                parts.push(interactions.clone());
            }
        }
        if let Some(ref topic) = self.topic_or_insight {
            if !topic.is_empty() {
                parts.push(topic.clone());
            }
        }
        if parts.is_empty() {
            None
        } else {
            Some(parts.join("\n\n"))
        }
    }

    /// Combine all context sections into a single string for prompt injection.
    /// This replaces the manual string concatenation in `process_message()`.
    pub fn to_prompt_string(&self) -> Option<String> {
        let mut parts = Vec::new();

        if let Some(ref card) = self.peer_card {
            if !card.is_empty() {
                parts.push(card.clone());
            }
        }

        if let Some(ref rep) = self.peer_representation {
            if !rep.is_empty() {
                parts.push(rep.clone());
            }
        }

        if let Some(ref interactions) = self.interactions {
            if !interactions.is_empty() {
                parts.push(interactions.clone());
            }
        }

        if let Some(ref topic) = self.topic_or_insight {
            if !topic.is_empty() {
                parts.push(topic.clone());
            }
        }

        if parts.is_empty() {
            None
        } else {
            Some(parts.join("\n\n"))
        }
    }
}

// ============================================================================
// Context Builder
// ============================================================================

/// Build a complete session context by combining existing RAG pipelines with
/// Honcho-style observation retrieval.
///
/// This is the single entry point that `process_message()` should call instead
/// of the current inline RAG assembly block.
pub async fn build_session_context<R: Runtime>(
    app_handle: &AppHandle<R>,
    _http_client: &reqwest::Client, // reserved for future embedding generation
    _config: &crate::config::AppConfig, // reserved for future budget tuning
    query: &str,
    query_embedding: &[f32],
) -> SessionContext {
    // 1. Existing: interaction search (BM25 + dense + RRF)
    let interactions = {
        let relevant = crate::interactions::hybrid_search_interactions(
            app_handle,
            query,
            query_embedding,
            5, // limit
        )
        .unwrap_or_default();

        if relevant.is_empty() {
            log::info!("[Context] No relevant past interactions found");
            None
        } else {
            log::info!(
                "[Context] Found {} relevant past interactions",
                relevant.len()
            );
            let mut s = String::from("\n\nRelevant Past Interactions:\n");
            for entry in relevant {
                s.push_str(&format!(
                    "- [{}] {}: {}\n",
                    entry.ts.format("%Y-%m-%d"),
                    entry.role,
                    entry.content
                ));
            }
            Some(s)
        }
    };

    // 2. Existing: topic/insight retrieval (vector + FTS5)
    let topic_or_insight = {
        let handle = app_handle.clone();
        let msg = query.to_string();
        let embedding = query_embedding.to_vec();
        let context_res = match tokio::task::spawn_blocking(move || {
            crate::memories::find_relevant_context(&handle, &msg, &embedding)
        })
        .await
        {
            Ok(res) => res,
            Err(e) => {
                log::error!("[Context] Context lookup task panicked: {}", e);
                Ok(None)
            }
        };

        match context_res {
            Ok(Some((name, content, is_insight))) => {
                if is_insight {
                    log::info!("[Context] Using insight: {}", name);
                    Some(format!(
                        "\n\nRelevant Insight:\n### Insight: {}\n{}\n\n",
                        name, content
                    ))
                } else {
                    log::info!("[Context] Using topic: {}", name);
                    Some(format!(
                        "\n\nRelevant Topic Summary:\n### Topic: {}\n{}\n\n",
                        name, content
                    ))
                }
            }
            Ok(None) => {
                log::info!("[Context] No relevant topic or insight found");
                None
            }
            Err(e) => {
                log::warn!("[Context] Topic/insight lookup failed: {}", e);
                None
            }
        }
    };

    // 3. NEW: Observation-based user representation (Honcho-style)
    let (peer_representation, peer_card) = {
        let handle = app_handle.clone();
        let embedding = query_embedding.to_vec();

        match tokio::task::spawn_blocking(move || build_observation_context(&handle, &embedding))
            .await
        {
            Ok(Ok((rep, card))) => (rep, card),
            Ok(Err(e)) => {
                log::warn!("[Context] Observation context failed: {}", e);
                (None, None)
            }
            Err(e) => {
                log::error!("[Context] Observation task panicked: {}", e);
                (None, None)
            }
        }
    };

    log::info!(
        "[Context] Assembled: interactions={}, topic/insight={}, peer_card={}, peer_rep={}",
        interactions.is_some(),
        topic_or_insight.is_some(),
        peer_card.is_some(),
        peer_representation.is_some(),
    );

    SessionContext {
        interactions,
        topic_or_insight,
        peer_representation,
        peer_card,
    }
}

/// Build observation-based context on a blocking thread (SQLite is sync).
fn build_observation_context<R: Runtime>(
    app_handle: &AppHandle<R>,
    query_embedding: &[f32],
) -> Result<(Option<String>, Option<String>), String> {
    let store = crate::memories::get_vector_store(app_handle)?;

    // Peer card
    let card_str = match crate::observations::get_peer_card(&store, "shard", "user")? {
        Some(card) if !card.facts.is_empty() => Some(crate::observations::format_peer_card(&card)),
        _ => None,
    };

    // Skip expensive search if no observations exist yet
    let obs_count = crate::observations::count_observations(&store, "user")?;
    if obs_count == 0 {
        return Ok((None, card_str));
    }

    // Working representation (blended: semantic + top-derived + recent)
    let observations = crate::observations::get_working_representation(
        &store,
        "user",
        query_embedding,
        15, // total budget
    )?;

    let rep_str = if observations.is_empty() {
        None
    } else {
        Some(crate::observations::format_observations_as_markdown(
            &observations,
        ))
    };

    Ok((rep_str, card_str))
}
