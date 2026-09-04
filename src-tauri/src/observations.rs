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

use crate::dedup::{is_duplicate, DedupKind, DEFAULT_WINDOW_SECS};
use crate::vector_store::{compute_content_hash, VectorStore};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::time::Duration;

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

    pub fn parse_level(s: &str) -> Option<Self> {
        match s {
            "explicit" => Some(Self::Explicit),
            "deductive" => Some(Self::Deductive),
            "inductive" => Some(Self::Inductive),
            "contradiction" => Some(Self::Contradiction),
            _ => None,
        }
    }
}

/// Phase 2.2 — Semantic edge type linking a derived observation back to its
/// source(s). `Derived` is the legacy "this fact implies that fact" relation
/// (NULL in the DB for back-compat). The other variants carry causal /
/// referential meaning that matters once Shard starts editing its own code:
///
/// * `Modifies(file)` — an observation about editing a file
/// * `Causes` — the parent observation's outcome led to this one
/// * `Fixes` — this observation reports a fix to an earlier issue
/// * `Contradicts` — flagged conflict; combined with `tvalid_end` on the
///   superseded row this is how "fact X used to be true, now Y is" is modeled
/// * `DependsOn` / `Uses` — referential edges for code/persona/tool linkage
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    Derived,
    Modifies,
    Causes,
    Fixes,
    Contradicts,
    DependsOn,
    Uses,
}

impl EdgeKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Derived => "derived",
            Self::Modifies => "modifies",
            Self::Causes => "causes",
            Self::Fixes => "fixes",
            Self::Contradicts => "contradicts",
            Self::DependsOn => "depends_on",
            Self::Uses => "uses",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "derived" => Some(Self::Derived),
            "modifies" => Some(Self::Modifies),
            "causes" => Some(Self::Causes),
            "fixes" => Some(Self::Fixes),
            "contradicts" => Some(Self::Contradicts),
            "depends_on" => Some(Self::DependsOn),
            "uses" => Some(Self::Uses),
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
    /// Phase 2.2 — semantic edge linking this observation to its sources.
    /// `None` = legacy `Derived` (pre-Phase-2 rows).
    pub edge_kind: Option<EdgeKind>,
    /// Phase 2.2 — temporal validity. Defaults to `created_at` on insert.
    pub tvalid_start: Option<String>,
    /// Phase 2.2 — null while still valid; set by `supersede` when a newer
    /// observation contradicts this one.
    pub tvalid_end: Option<String>,
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

/// Insert an observation only if its content hash has not been seen by the
/// dedup window in the last `DEFAULT_WINDOW_SECS` seconds. Returns `true`
/// when an insert actually happened, `false` when the dedup window
/// suppressed it.
///
/// Phase 1.2 wrapper around [`insert_observation`]. Existing call sites can
/// migrate incrementally; the underlying primitive remains usable when a
/// caller explicitly wants to bypass dedup (e.g. background re-derivation
/// jobs that re-record an existing fact intentionally).
pub fn insert_observation_dedup(
    store: &VectorStore,
    obs: &Observation,
    embedding: Option<&[f32]>,
) -> Result<bool, String> {
    if is_duplicate(
        store,
        &obs.content_hash,
        DedupKind::Observation,
        Duration::from_secs(DEFAULT_WINDOW_SECS),
    ) {
        return Ok(false);
    }
    insert_observation(store, obs, embedding)?;
    Ok(true)
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
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
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
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
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
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
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

    // sqlite-vec KNN search over embeddings, with a JOIN to fully hydrate observations
    // in a single pass (distance is used only for filtering/ordering, not returned)
    let mut stmt = store
        .conn
        .prepare(
            "SELECT obs.id, obs.observer, obs.observed, obs.content, obs.level, \
                    obs.source_ids, obs.times_derived, obs.session_name, \
                    obs.content_hash, obs.created_at, obs.deleted_at \
             FROM observation_embeddings v \
             JOIN observations obs ON v.observation_id = obs.id \
             WHERE v.embedding MATCH ?1 AND k = ?2 AND v.distance <= ?3 \
                   AND obs.observed = ?4 AND obs.deleted_at IS NULL \
             ORDER BY v.distance \
             LIMIT ?5",
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map(
            params![
                embedding_bytes,
                limit as i64 * 2,
                max_distance,
                observed,
                limit as i64
            ],
            |row| Ok(row_to_observation(row)),
        )
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
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

    // Joined with observations table to avoid N+1 hydrate calls
    let mut stmt = store
        .conn
        .prepare(
            "SELECT obs.id, obs.observer, obs.observed, obs.content, obs.level, \
                    obs.source_ids, obs.times_derived, obs.session_name, \
                    obs.content_hash, obs.created_at, obs.deleted_at \
             FROM observations_fts \
             JOIN observations obs ON observations_fts.observation_id = obs.id \
             WHERE observations_fts MATCH ?1 \
                   AND obs.observed = ?2 AND obs.deleted_at IS NULL \
             ORDER BY bm25(observations_fts) \
             LIMIT ?3",
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map(params![sanitized, observed, limit as i64], |row| {
            Ok(row_to_observation(row))
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
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

    let semantic =
        search_observations_by_embedding(store, observed, query_embedding, per_bucket, 0.45)?;
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
    let explicit: Vec<_> = observations
        .iter()
        .filter(|o| o.level == ObservationLevel::Explicit)
        .collect();
    let deductive: Vec<_> = observations
        .iter()
        .filter(|o| o.level == ObservationLevel::Deductive)
        .collect();
    let inductive: Vec<_> = observations
        .iter()
        .filter(|o| o.level == ObservationLevel::Inductive)
        .collect();

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
            let facts: Vec<String> = serde_json::from_str(&facts_json).unwrap_or_default();
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

/// Convert an `observations` row (in the canonical column order used by
/// every SELECT in this module) into an [`Observation`]. Columns added in
/// Phase 2.2 (`edge_kind`, `tvalid_start`, `tvalid_end`) are read by name
/// via the secondary SELECT in [`hydrate_phase2_fields`] when present —
/// keeping every existing SELECT statement untouched.
fn row_to_observation(row: &rusqlite::Row) -> Observation {
    let source_ids_json: String = row.get(5).unwrap_or_else(|_| "[]".to_string());
    let source_ids: Vec<String> = serde_json::from_str(&source_ids_json).unwrap_or_default();

    Observation {
        id: row.get(0).unwrap_or_default(),
        observer: row.get(1).unwrap_or_default(),
        observed: row.get(2).unwrap_or_default(),
        content: row.get(3).unwrap_or_default(),
        level: ObservationLevel::parse_level(&row.get::<_, String>(4).unwrap_or_default())
            .unwrap_or(ObservationLevel::Explicit),
        source_ids,
        times_derived: row.get::<_, i64>(6).unwrap_or(0) as u32,
        session_name: row.get(7).ok(),
        content_hash: row.get(8).unwrap_or_default(),
        created_at: row.get(9).unwrap_or_default(),
        deleted_at: row.get(10).ok(),
        edge_kind: None,
        tvalid_start: None,
        tvalid_end: None,
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

// ============================================================================
// Phase 1.3 — Decay & eviction
// ============================================================================
//
// Each observation carries a `decay_score` in [0.0, 1.0] that decays on an
// Ebbinghaus-style curve with a 15-day half-life, modulated upward by
// `times_derived`. Access (`touch_observation`) reinforces the score with a
// 1.0 ceiling. `decay_sweep` soft-deletes rows below `evict_threshold` and
// (optionally) hard-deletes already-soft-deleted rows older than the grace
// period. All math lives in pure helpers so the curve can be unit-tested
// without a SQLite round-trip.

/// Half-life in days for the Ebbinghaus decay curve.
pub const DECAY_HALF_LIFE_DAYS: f32 = 15.0;
/// Score below which an observation is a candidate for soft-deletion.
pub const DEFAULT_EVICT_THRESHOLD: f32 = 0.05;
/// Reinforcement boost applied per `touch_observation` call.
const TOUCH_BOOST: f32 = 0.25;

/// Pure decay function: returns the score for a fresh observation aged
/// `age_days` with `times_derived` derivations. Independent of SQLite so it
/// can be unit-tested.
pub fn decay_score(age_days: f32, times_derived: u32) -> f32 {
    if age_days <= 0.0 {
        return 1.0;
    }
    let half_life = DECAY_HALF_LIFE_DAYS.max(0.001);
    let base = (-age_days * std::f32::consts::LN_2 / half_life).exp();
    let boost = 1.0 + (times_derived as f32 + 1.0).ln() * 0.15;
    (base * boost).clamp(0.0, 1.0)
}

/// Bump `last_accessed` to now and reinforce `decay_score` (capped at 1.0).
/// Returns the new score, or `None` if the observation is missing/deleted.
pub fn touch_observation(store: &VectorStore, id: &str) -> Result<Option<f32>, String> {
    let now = chrono::Utc::now().to_rfc3339();
    let existing: Option<f32> = store
        .conn
        .query_row(
            "SELECT decay_score FROM observations WHERE id = ? AND deleted_at IS NULL",
            params![id],
            |row| row.get::<_, f64>(0).map(|v| v as f32),
        )
        .optional()
        .map_err(|e| format!("touch lookup failed: {}", e))?;

    let Some(prev) = existing else {
        return Ok(None);
    };
    let next = (prev + TOUCH_BOOST).clamp(0.0, 1.0);
    store
        .conn
        .execute(
            "UPDATE observations SET last_accessed = ?, decay_score = ? WHERE id = ?",
            params![now, next as f64, id],
        )
        .map_err(|e| format!("touch update failed: {}", e))?;
    Ok(Some(next))
}

/// Recompute `decay_score` for every live observation based on age and
/// `times_derived`. Returns the number of rows updated.
///
/// The curve is computed in Rust (rather than SQL `exp()`/`log()`) so we
/// don't have to depend on `SQLITE_ENABLE_MATH_FUNCTIONS`, which isn't
/// enabled by default in the bundled rusqlite build. Performed inside a
/// single transaction so a 10k-row sweep still completes in <50 ms.
pub fn recompute_decay(store: &VectorStore) -> Result<usize, String> {
    use chrono::DateTime;

    let now = chrono::Utc::now();

    // Snapshot the rows up front. The id/timestamp/times_derived tuple is
    // ~64 bytes per row; 10k rows = ~640KB which is trivial.
    let rows: Vec<(String, String, u32)> = {
        let mut stmt = store
            .conn
            .prepare(
                "SELECT id, COALESCE(last_accessed, created_at), times_derived \
                 FROM observations WHERE deleted_at IS NULL",
            )
            .map_err(|e| e.to_string())?;
        let collected: Vec<(String, String, u32)> = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)? as u32,
                ))
            })
            .map_err(|e| e.to_string())?
            .filter_map(|r| r.ok())
            .collect();
        collected
    };

    let tx = store
        .conn
        .unchecked_transaction()
        .map_err(|e| format!("recompute_decay tx begin failed: {}", e))?;
    {
        let mut stmt = tx
            .prepare("UPDATE observations SET decay_score = ? WHERE id = ?")
            .map_err(|e| e.to_string())?;
        for (id, ts, times_derived) in &rows {
            let age_days = DateTime::parse_from_rfc3339(ts)
                .map(|t| {
                    let secs =
                        (now - t.with_timezone(&chrono::Utc)).num_milliseconds() as f32 / 1000.0;
                    (secs / 86_400.0).max(0.0)
                })
                .unwrap_or(0.0);
            let score = decay_score(age_days, *times_derived);
            stmt.execute(params![score as f64, id])
                .map_err(|e| format!("recompute_decay update failed: {}", e))?;
        }
    }
    tx.commit()
        .map_err(|e| format!("recompute_decay tx commit failed: {}", e))?;
    Ok(rows.len())
}

/// Soft-delete every live observation whose `decay_score` is below
/// `threshold`. Returns the number of rows affected.
pub fn decay_sweep(store: &VectorStore, threshold: f32) -> Result<usize, String> {
    let now = chrono::Utc::now().to_rfc3339();
    let n = store
        .conn
        .execute(
            "UPDATE observations SET deleted_at = ? \
             WHERE deleted_at IS NULL AND decay_score < ?",
            params![now, threshold as f64],
        )
        .map_err(|e| format!("decay_sweep failed: {}", e))?;
    Ok(n)
}

/// Hard-delete observations soft-deleted longer than `grace`. Also removes
/// their embedding rows so vec0 doesn't accumulate dead vectors.
pub fn hard_delete_expired(store: &VectorStore, grace: chrono::Duration) -> Result<usize, String> {
    let cutoff = (chrono::Utc::now() - grace).to_rfc3339();

    // First collect the ids so we can also delete from observation_embeddings.
    let ids: Vec<String> = {
        let mut stmt = store
            .conn
            .prepare("SELECT id FROM observations WHERE deleted_at IS NOT NULL AND deleted_at < ?")
            .map_err(|e| e.to_string())?;
        let collected: Vec<String> = stmt
            .query_map(params![cutoff], |row| row.get::<_, String>(0))
            .map_err(|e| e.to_string())?
            .filter_map(|r| r.ok())
            .collect();
        collected
    };

    let mut removed = 0usize;
    for id in &ids {
        let _ = store.conn.execute(
            "DELETE FROM observation_embeddings WHERE observation_id = ?",
            params![id],
        );
        removed += store
            .conn
            .execute("DELETE FROM observations WHERE id = ?", params![id])
            .map_err(|e| format!("hard_delete failed: {}", e))?;
    }
    Ok(removed)
}

/// Create a new observation with auto-generated ID and content hash.
pub fn make_observation(
    content: &str,
    level: ObservationLevel,
    source_ids: Vec<String>,
    session_name: Option<String>,
) -> Observation {
    let now = chrono::Utc::now().to_rfc3339();
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
        created_at: now.clone(),
        deleted_at: None,
        edge_kind: None,
        tvalid_start: Some(now),
        tvalid_end: None,
    }
}

// ============================================================================
// Phase 2.2 — Typed edges + temporal validity
// ============================================================================

/// Insert an observation and tag the link to its sources with a semantic
/// `EdgeKind`. Equivalent to calling [`insert_observation`] and then setting
/// `edge_kind` directly in SQL, in one round-trip.
pub fn insert_with_edge(
    store: &VectorStore,
    obs: &Observation,
    embedding: Option<&[f32]>,
    edge: EdgeKind,
) -> Result<(), String> {
    insert_observation(store, obs, embedding)?;
    store
        .conn
        .execute(
            "UPDATE observations SET edge_kind = ? WHERE id = ?",
            params![edge.as_str(), obs.id],
        )
        .map_err(|e| format!("insert_with_edge: set edge_kind failed: {}", e))?;
    Ok(())
}

/// Mark `old_id` as superseded by `new_obs`: closes its `tvalid_end` to
/// "now" and inserts `new_obs` with a `Contradicts` edge pointing at the
/// old row. Used by the deriver/dream pipeline when a fresh fact directly
/// conflicts with one already on file.
pub fn supersede(
    store: &VectorStore,
    old_id: &str,
    new_obs: &mut Observation,
    embedding: Option<&[f32]>,
) -> Result<(), String> {
    let now = chrono::Utc::now().to_rfc3339();
    store
        .conn
        .execute(
            "UPDATE observations SET tvalid_end = ? WHERE id = ? AND tvalid_end IS NULL",
            params![now, old_id],
        )
        .map_err(|e| format!("supersede: set tvalid_end failed: {}", e))?;

    // Wire the new observation's source_ids + edge_kind so callers don't
    // have to remember to do it.
    if !new_obs.source_ids.iter().any(|s| s == old_id) {
        new_obs.source_ids.push(old_id.to_string());
    }
    insert_with_edge(store, new_obs, embedding, EdgeKind::Contradicts)
}

/// Walk the source DAG from a starting observation up to `max_depth` levels,
/// returning every ancestor in BFS order. Soft-deleted ancestors are
/// included so the chain stays auditable; callers filter as needed.
pub fn causal_chain(
    store: &VectorStore,
    start_id: &str,
    max_depth: usize,
) -> Result<Vec<Observation>, String> {
    use std::collections::VecDeque;

    type ObservationRow = (
        String,
        String,
        String,
        String,
        String,
        String,
        i64,
        Option<String>,
        String,
        String,
        Option<String>,
    );

    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    let mut frontier: VecDeque<(String, usize)> = VecDeque::new();
    frontier.push_back((start_id.to_string(), 0));

    while let Some((id, depth)) = frontier.pop_front() {
        if !seen.insert(id.clone()) {
            continue;
        }
        if depth > max_depth {
            continue;
        }
        // Hydrate the row itself.
        let row: Option<ObservationRow> = store
            .conn
            .query_row(
                "SELECT id, observer, observed, content, level, source_ids, times_derived, \
                            session_name, content_hash, created_at, deleted_at \
                     FROM observations WHERE id = ?",
                params![id],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                        r.get(7)?,
                        r.get(8)?,
                        r.get(9)?,
                        r.get(10)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let Some(tuple) = row else { continue };
        let source_ids: Vec<String> = serde_json::from_str(&tuple.5).unwrap_or_default();
        // Skip the starting row itself in the output — `causal_chain`
        // returns ancestors, not the seed.
        if depth > 0 {
            out.push(Observation {
                id: tuple.0,
                observer: tuple.1,
                observed: tuple.2,
                content: tuple.3,
                level: ObservationLevel::parse_level(&tuple.4)
                    .unwrap_or(ObservationLevel::Explicit),
                source_ids: source_ids.clone(),
                times_derived: tuple.6 as u32,
                session_name: tuple.7,
                content_hash: tuple.8,
                created_at: tuple.9,
                deleted_at: tuple.10,
                edge_kind: None,
                tvalid_start: None,
                tvalid_end: None,
            });
        }
        for parent in source_ids {
            frontier.push_back((parent, depth + 1));
        }
    }
    Ok(out)
}

/// Return live observations whose temporal validity window includes "now".
/// Excludes soft-deleted rows AND rows that have been superseded
/// (`tvalid_end` set in the past). Sorted by `created_at` DESC.
pub fn currently_valid(
    store: &VectorStore,
    observed: &str,
    limit: usize,
) -> Result<Vec<Observation>, String> {
    let now = chrono::Utc::now().to_rfc3339();
    let mut stmt = store
        .conn
        .prepare(
            "SELECT id, observer, observed, content, level, source_ids, times_derived, \
                    session_name, content_hash, created_at, deleted_at \
             FROM observations \
             WHERE observed = ? AND deleted_at IS NULL \
                   AND (tvalid_end IS NULL OR tvalid_end > ?) \
             ORDER BY created_at DESC LIMIT ?",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![observed, now, limit as i64], |row| {
            Ok(row_to_observation(row))
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}
