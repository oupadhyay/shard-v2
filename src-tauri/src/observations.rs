//! Honcho-style Peer-Centric Observations
//!
//! Provides CRUD operations and hybrid retrieval (vector + FTS5) for
//! leveled observations about entities (typically the user).
//!
//! Observation levels form a DAG:
//! - `Explicit`: Direct facts extracted from messages
//! - `Deductive`: Logical implications derived from explicit observations
//! - `Inductive`: Patterns identified across multiple observations
//! - `Contradiction`: Flagged conflicts between observations

use crate::vector_store::{compute_content_hash, VectorStore};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

// ============================================================================
// Types
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ObservationLevel {
    Explicit,
    Deductive,
    Inductive,
    Contradiction,
}

impl ObservationLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Explicit => "explicit",
            Self::Deductive => "deductive",
            Self::Inductive => "inductive",
            Self::Contradiction => "contradiction",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "explicit" => Some(Self::Explicit),
            "deductive" => Some(Self::Deductive),
            "inductive" => Some(Self::Inductive),
            "contradiction" => Some(Self::Contradiction),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Observation {
    pub id: String,
    pub observer: String,
    pub observed: String,
    pub content: String,
    pub level: ObservationLevel,
    /// Parent observation IDs (DAG edges). Empty for explicit observations.
    pub source_ids: Vec<String>,
    /// How many times this observation has been used as a source for derived observations.
    pub times_derived: u32,
    pub session_name: Option<String>,
    pub content_hash: String,
    pub created_at: String,
    pub deleted_at: Option<String>,
}

/// A curated list of biographical facts about an observed entity.
/// Mirrors Honcho's `Collection.internal_metadata.peer_card`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PeerCard {
    pub observer: String,
    pub observed: String,
    pub facts: Vec<String>,
    pub updated_at: String,
}

// ============================================================================
// Write Operations
// ============================================================================

/// Insert a new observation into the store. Also inserts its embedding if provided.
pub fn insert_observation(
    store: &VectorStore,
    obs: &Observation,
    embedding: Option<&[f32]>,
) -> Result<(), String> {
    let source_ids_json =
        serde_json::to_string(&obs.source_ids).unwrap_or_else(|_| "[]".to_string());

    store
        .conn
        .execute(
            "INSERT OR IGNORE INTO observations \
             (id, observer, observed, content, level, source_ids, times_derived, \
              session_name, content_hash, created_at, deleted_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                obs.id,
                obs.observer,
                obs.observed,
                obs.content,
                obs.level.as_str(),
                source_ids_json,
                obs.times_derived,
                obs.session_name,
                obs.content_hash,
                obs.created_at,
                obs.deleted_at,
            ],
        )
        .map_err(|e| format!("Failed to insert observation: {}", e))?;

    // Insert embedding into vec0 table
    if let Some(emb) = embedding {
        let embedding_bytes = f32_vec_to_bytes(emb);
        store
            .conn
            .execute(
                "INSERT OR REPLACE INTO observation_embeddings (observation_id, embedding) VALUES (?1, ?2)",
                params![obs.id, embedding_bytes],
            )
            .map_err(|e| format!("Failed to insert observation embedding: {}", e))?;
    }

    // Increment times_derived on all source observations
    for source_id in &obs.source_ids {
        store
            .conn
            .execute(
                "UPDATE observations SET times_derived = times_derived + 1 WHERE id = ?",
                params![source_id],
            )
            .map_err(|e| format!("Failed to increment times_derived: {}", e))?;
    }

    Ok(())
}

/// Soft-delete an observation (set deleted_at timestamp).
pub fn soft_delete_observation(store: &VectorStore, id: &str) -> Result<(), String> {
    let now = chrono::Utc::now().to_rfc3339();
    store
        .conn
        .execute(
            "UPDATE observations SET deleted_at = ? WHERE id = ? AND deleted_at IS NULL",
            params![now, id],
        )
        .map_err(|e| format!("Failed to soft-delete observation: {}", e))?;
    Ok(())
}

// ============================================================================
// Read Operations
// ============================================================================

/// Get the total number of non-deleted observations for an observed entity.
pub fn count_observations(store: &VectorStore, observed: &str) -> Result<usize, String> {
    let count: i64 = store
        .conn
        .query_row(
            "SELECT COUNT(*) FROM observations WHERE observed = ? AND deleted_at IS NULL",
            params![observed],
            |row| row.get(0),
        )
        .map_err(|e| format!("Failed to count observations: {}", e))?;
    Ok(count as usize)
}

/// Get observations by level for an observed entity.
pub fn get_observations_by_level(
    store: &VectorStore,
    observed: &str,
    level: ObservationLevel,
    limit: usize,
) -> Result<Vec<Observation>, String> {
    let mut stmt = store
        .conn
        .prepare(
            "SELECT id, observer, observed, content, level, source_ids, times_derived, \
             session_name, content_hash, created_at, deleted_at \
             FROM observations \
             WHERE observed = ? AND level = ? AND deleted_at IS NULL \
             ORDER BY created_at DESC LIMIT ?",
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map(params![observed, level.as_str(), limit as i64], |row| {
            Ok(row_to_observation(row))
        })
        .map_err(|e| e.to_string())?;

    let mut result = Vec::new();
    for row in rows {
        result.push(row.map_err(|e| e.to_string())?);
    }
    Ok(result)
}

/// Get the N most-derived observations (highest `times_derived`).
/// Honcho equivalent: "most referenced" observations for the representation blend.
pub fn get_top_derived_observations(
    store: &VectorStore,
    observed: &str,
    limit: usize,
) -> Result<Vec<Observation>, String> {
    let mut stmt = store
        .conn
        .prepare(
            "SELECT id, observer, observed, content, level, source_ids, times_derived, \
             session_name, content_hash, created_at, deleted_at \
             FROM observations \
             WHERE observed = ? AND deleted_at IS NULL AND times_derived > 0 \
             ORDER BY times_derived DESC, created_at DESC LIMIT ?",
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map(params![observed, limit as i64], |row| {
            Ok(row_to_observation(row))
        })
        .map_err(|e| e.to_string())?;

    let mut result = Vec::new();
    for row in rows {
        result.push(row.map_err(|e| e.to_string())?);
    }
    Ok(result)
}

/// Get the N most recent observations.
pub fn get_recent_observations(
    store: &VectorStore,
    observed: &str,
    limit: usize,
) -> Result<Vec<Observation>, String> {
    let mut stmt = store
        .conn
        .prepare(
            "SELECT id, observer, observed, content, level, source_ids, times_derived, \
             session_name, content_hash, created_at, deleted_at \
             FROM observations \
             WHERE observed = ? AND deleted_at IS NULL \
             ORDER BY created_at DESC LIMIT ?",
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map(params![observed, limit as i64], |row| {
            Ok(row_to_observation(row))
        })
        .map_err(|e| e.to_string())?;

    let mut result = Vec::new();
    for row in rows {
        result.push(row.map_err(|e| e.to_string())?);
    }
    Ok(result)
}

// ============================================================================
// Hybrid Search (Vector + FTS5) — mirrors Honcho's semantic search
// ============================================================================

/// KNN search over observation embeddings, filtered to a specific observed entity.
pub fn search_observations_by_embedding(
    store: &VectorStore,
    observed: &str,
    query_embedding: &[f32],
    limit: usize,
    max_distance: f32,
) -> Result<Vec<Observation>, String> {
    let embedding_bytes = f32_vec_to_bytes(query_embedding);

    // sqlite-vec KNN search — returns observation_id + distance
    let mut stmt = store
        .conn
        .prepare(
            "SELECT observation_id, distance \
             FROM observation_embeddings \
             WHERE embedding MATCH ?1 AND k = ?2 \
             ORDER BY distance",
        )
        .map_err(|e| e.to_string())?;

    let candidates: Vec<(String, f32)> = stmt
        .query_map(params![embedding_bytes, limit as i64 * 2], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, f32>(1)?))
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .filter(|(_, dist)| *dist <= max_distance)
        .collect();

    // Hydrate + filter by observed entity and soft-delete
    let mut results = Vec::new();
    for (obs_id, _) in candidates {
        if results.len() >= limit {
            break;
        }
        if let Some(obs) = get_observation_by_id(store, &obs_id)? {
            if obs.observed == observed && obs.deleted_at.is_none() {
                results.push(obs);
            }
        }
    }

    Ok(results)
}

/// FTS5 keyword search over observations.
pub fn search_observations_by_keyword(
    store: &VectorStore,
    observed: &str,
    query: &str,
    limit: usize,
) -> Result<Vec<Observation>, String> {
    // Sanitize query for FTS5
    let sanitized = sanitize_fts5_query(query);
    if sanitized.is_empty() {
        return Ok(Vec::new());
    }

    let mut stmt = store
        .conn
        .prepare(
            "SELECT observation_id \
             FROM observations_fts \
             WHERE observations_fts MATCH ?1 \
             ORDER BY bm25(observations_fts) \
             LIMIT ?2",
        )
        .map_err(|e| e.to_string())?;

    let candidates: Vec<String> = stmt
        .query_map(params![sanitized, limit as i64 * 2], |row| {
            row.get::<_, String>(0)
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();

    let mut results = Vec::new();
    for obs_id in candidates {
        if results.len() >= limit {
            break;
        }
        if let Some(obs) = get_observation_by_id(store, &obs_id)? {
            if obs.observed == observed && obs.deleted_at.is_none() {
                results.push(obs);
            }
        }
    }

    Ok(results)
}

// ============================================================================
// Working Representation — Honcho's blended retrieval
// ============================================================================

/// Build a "working representation" of an observed entity by blending three
/// retrieval strategies (mirrors Honcho's `get_working_representation`):
///
/// 1. **Semantic**: KNN search using the query embedding
/// 2. **Top-derived**: Observations with highest `times_derived` (most referenced)
/// 3. **Recent**: Most recently created observations
///
/// Results are merged and deduplicated.
pub fn get_working_representation(
    store: &VectorStore,
    observed: &str,
    query_embedding: &[f32],
    total_budget: usize,
) -> Result<Vec<Observation>, String> {
    let per_bucket = (total_budget / 3).max(1);

    let semantic = search_observations_by_embedding(store, observed, query_embedding, per_bucket, 0.45)?;
    let top = get_top_derived_observations(store, observed, per_bucket)?;
    let recent = get_recent_observations(store, observed, per_bucket)?;

    // Merge + deduplicate by ID
    let mut seen = HashSet::new();
    let mut result = Vec::with_capacity(total_budget);
    for obs in semantic.into_iter().chain(top).chain(recent) {
        if seen.insert(obs.id.clone()) {
            result.push(obs);
        }
        if result.len() >= total_budget {
            break;
        }
    }

    Ok(result)
}

/// Format observations as markdown for prompt injection.
pub fn format_observations_as_markdown(observations: &[Observation]) -> String {
    if observations.is_empty() {
        return String::new();
    }

    let mut out = String::from("## User Profile (Observations)\n\n");

    // Group by level for structured output
    let explicit: Vec<_> = observations.iter().filter(|o| o.level == ObservationLevel::Explicit).collect();
    let deductive: Vec<_> = observations.iter().filter(|o| o.level == ObservationLevel::Deductive).collect();
    let inductive: Vec<_> = observations.iter().filter(|o| o.level == ObservationLevel::Inductive).collect();

    if !inductive.is_empty() {
        out.push_str("**Patterns & Traits:**\n");
        for obs in &inductive {
            out.push_str(&format!("- {}\n", obs.content));
        }
        out.push('\n');
    }

    if !deductive.is_empty() {
        out.push_str("**Inferred:**\n");
        for obs in &deductive {
            out.push_str(&format!("- {}\n", obs.content));
        }
        out.push('\n');
    }

    if !explicit.is_empty() {
        out.push_str("**Known Facts:**\n");
        for obs in &explicit {
            out.push_str(&format!("- {}\n", obs.content));
        }
        out.push('\n');
    }

    out
}

// ============================================================================
// Peer Card Operations
// ============================================================================

/// Get the peer card for an observer×observed pair.
pub fn get_peer_card(
    store: &VectorStore,
    observer: &str,
    observed: &str,
) -> Result<Option<PeerCard>, String> {
    let result: Option<(String, String)> = store
        .conn
        .query_row(
            "SELECT facts, updated_at FROM peer_cards WHERE observer = ? AND observed = ?",
            params![observer, observed],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;

    match result {
        Some((facts_json, updated_at)) => {
            let facts: Vec<String> =
                serde_json::from_str(&facts_json).unwrap_or_default();
            Ok(Some(PeerCard {
                observer: observer.to_string(),
                observed: observed.to_string(),
                facts,
                updated_at,
            }))
        }
        None => Ok(None),
    }
}

/// Upsert the peer card for an observer×observed pair.
pub fn upsert_peer_card(
    store: &VectorStore,
    observer: &str,
    observed: &str,
    facts: &[String],
) -> Result<(), String> {
    let facts_json = serde_json::to_string(facts).unwrap_or_else(|_| "[]".to_string());
    let now = chrono::Utc::now().to_rfc3339();

    store
        .conn
        .execute(
            "INSERT INTO peer_cards (observer, observed, facts, updated_at) \
             VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT(observer, observed) DO UPDATE SET facts = ?3, updated_at = ?4",
            params![observer, observed, facts_json, now],
        )
        .map_err(|e| format!("Failed to upsert peer card: {}", e))?;

    Ok(())
}

/// Format peer card facts for prompt injection.
pub fn format_peer_card(card: &PeerCard) -> String {
    if card.facts.is_empty() {
        return String::new();
    }
    let mut out = String::from("## User Card\n\n");
    for fact in &card.facts {
        out.push_str(&format!("- {}\n", fact));
    }
    out
}

// ============================================================================
// Helpers
// ============================================================================

fn get_observation_by_id(
    store: &VectorStore,
    id: &str,
) -> Result<Option<Observation>, String> {
    let result = store
        .conn
        .query_row(
            "SELECT id, observer, observed, content, level, source_ids, times_derived, \
             session_name, content_hash, created_at, deleted_at \
             FROM observations WHERE id = ?",
            params![id],
            |row| Ok(row_to_observation(row)),
        )
        .optional()
        .map_err(|e| e.to_string())?;

    Ok(result)
}

fn row_to_observation(row: &rusqlite::Row) -> Observation {
    let source_ids_json: String = row.get(5).unwrap_or_else(|_| "[]".to_string());
    let source_ids: Vec<String> =
        serde_json::from_str(&source_ids_json).unwrap_or_default();

    Observation {
        id: row.get(0).unwrap_or_default(),
        observer: row.get(1).unwrap_or_default(),
        observed: row.get(2).unwrap_or_default(),
        content: row.get(3).unwrap_or_default(),
        level: ObservationLevel::from_str(
            &row.get::<_, String>(4).unwrap_or_default(),
        )
        .unwrap_or(ObservationLevel::Explicit),
        source_ids,
        times_derived: row.get::<_, i64>(6).unwrap_or(0) as u32,
        session_name: row.get(7).ok(),
        content_hash: row.get(8).unwrap_or_default(),
        created_at: row.get(9).unwrap_or_default(),
        deleted_at: row.get(10).ok(),
    }
}

/// Convert f32 slice to bytes for sqlite-vec storage.
fn f32_vec_to_bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

/// Sanitize a query string for FTS5 (strip special operators).
fn sanitize_fts5_query(query: &str) -> String {
    let mut result = String::with_capacity(query.len());
    for c in query.chars() {
        match c {
            '+' | '{' | '}' | '(' | ')' | '^' | '~' | '*' | '"' => {
                result.push(' ');
            }
            _ => result.push(c),
        }
    }
    // Collapse whitespace and trim
    result.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Create a new observation with auto-generated ID and content hash.
pub fn make_observation(
    content: &str,
    level: ObservationLevel,
    source_ids: Vec<String>,
    session_name: Option<String>,
) -> Observation {
    Observation {
        id: uuid::Uuid::new_v4().to_string(),
        observer: "shard".to_string(),
        observed: "user".to_string(),
        content: content.to_string(),
        level,
        source_ids,
        times_derived: 0,
        session_name,
        content_hash: compute_content_hash(content),
        created_at: chrono::Utc::now().to_rfc3339(),
        deleted_at: None,
    }
}
