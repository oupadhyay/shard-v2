use crate::retrieval::{
    apply_temporal_boost, fuse_rrf_multi, load_bm25_index, min_dense_hits, rrf_k_default,
    temporal_tau_days, HitSource, ScoredHit,
};
/**
 * Interactions module - Logs full conversation history and provides RAG retrieval
 *
 * Implements Tier 3 of the memory system:
 * - Logs every turn to daily JSONL files
 * - Stores provider-generated embeddings alongside interaction entries
 * - Performs hybrid semantic/BM25 search for context retrieval
 */
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use tauri::{AppHandle, Manager, Runtime};

// ============================================================================
// Data Types
// ============================================================================

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct InteractionEntry {
    pub ts: DateTime<Utc>,
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding: Option<Vec<f32>>,
}

// ============================================================================
// Interaction Logging
// ============================================================================

fn get_interactions_dir<R: Runtime>(app_handle: &AppHandle<R>) -> Result<PathBuf, String> {
    let app_data_dir = app_handle
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to get app data dir: {}", e))?;

    let dir = app_data_dir.join("interactions");
    if !dir.exists() {
        fs::create_dir_all(&dir)
            .map_err(|e| format!("Failed to create interactions dir: {}", e))?;
    }
    Ok(dir)
}

fn get_today_log_path<R: Runtime>(app_handle: &AppHandle<R>) -> Result<PathBuf, String> {
    let dir = get_interactions_dir(app_handle)?;
    let today = Utc::now().format("%Y-%m-%d").to_string();
    Ok(dir.join(format!("interactions-{}.jsonl", today)))
}

pub async fn log_interaction<R: Runtime>(
    app_handle: &AppHandle<R>,
    role: &str,
    content: &str,
    embedding: Option<Vec<f32>>,
) -> Result<(), String> {
    let entry = InteractionEntry {
        ts: Utc::now(),
        role: role.to_string(),
        content: content.to_string(),
        embedding,
    };

    let path = get_today_log_path(app_handle)?;

    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("Failed to open interaction log: {}", e))?;

    let mut writer = std::io::BufWriter::new(file);
    let json = serde_json::to_string(&entry)
        .map_err(|e| format!("Failed to serialize interaction: {}", e))?;

    writeln!(writer, "{}", json).map_err(|e| format!("Failed to write interaction: {}", e))?;

    // Also update BM25 index for hybrid retrieval
    let doc_id = entry.ts.to_rfc3339();
    let mut bm25_index = crate::retrieval::load_bm25_index(app_handle)?;
    bm25_index.add_document(&doc_id, content);
    crate::retrieval::save_bm25_index(app_handle, &bm25_index)?;

    Ok(())
}

// ============================================================================
// RAG Retrieval
// ============================================================================

pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }

    let dot_product: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }

    dot_product / (norm_a * norm_b)
}

/// Search interactions by embedding similarity (dense-only search, kept as fallback)
#[allow(dead_code)]
pub fn search_interactions<R: Runtime>(
    app_handle: &AppHandle<R>,
    query_embedding: &[f32],
    limit: usize,
) -> Result<Vec<InteractionEntry>, String> {
    let dir = get_interactions_dir(app_handle)?;
    let mut results: Vec<(f32, InteractionEntry)> = Vec::new();

    // Read all jsonl files in the directory
    // In a production system, we'd use a proper vector DB or index,
    // but for <100k items, linear scan is acceptable.
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("jsonl") {
                if let Ok(file) = fs::File::open(path) {
                    let reader = BufReader::new(file);
                    #[allow(clippy::lines_filter_map_ok)]
                    for line in reader.lines().filter_map(Result::ok) {
                        if let Ok(entry) = serde_json::from_str::<InteractionEntry>(&line) {
                            if let Some(emb) = &entry.embedding {
                                let score = cosine_similarity(query_embedding, emb);
                                results.push((score, entry));
                            }
                        }
                    }
                }
            }
        }
    }

    // Sort by score descending
    results.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    // Return top K
    Ok(results
        .into_iter()
        .take(limit)
        .map(|(_, entry)| entry)
        .collect())
}

/// Hybrid search using RRF to fuse BM25 and dense retrieval results
///
/// Features:
/// - N-list RRF fusion (currently BM25 + dense interactions)
/// - Fallback to BM25-only when dense results are sparse
/// - Temporal boost for recency-sensitive queries
pub fn hybrid_search_interactions<R: Runtime>(
    app_handle: &AppHandle<R>,
    query: &str,
    query_embedding: &[f32],
    limit: usize,
) -> Result<Vec<InteractionEntry>, String> {
    // Get BM25 results (N = 50 candidates)
    let bm25_index = load_bm25_index(app_handle)?;
    let bm25_results = bm25_index.search(query, 50);

    // Convert BM25 results to ScoredHit
    let bm25_hits: Vec<ScoredHit> = bm25_results
        .iter()
        .map(|d| {
            let ts = chrono::DateTime::parse_from_rfc3339(&d.doc_id)
                .ok()
                .map(|dt| dt.with_timezone(&chrono::Utc));
            ScoredHit {
                doc_id: d.doc_id.clone(),
                score: d.score,
                source: HitSource::Bm25,
                ts,
            }
        })
        .collect();

    // Get dense results (N = 50 candidates)
    let dir = get_interactions_dir(app_handle)?;
    let mut dense_results: Vec<(f32, String, InteractionEntry)> = Vec::new();

    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("jsonl") {
                if let Ok(file) = fs::File::open(&path) {
                    let reader = BufReader::new(file);
                    #[allow(clippy::lines_filter_map_ok)]
                    for line in reader.lines().filter_map(Result::ok) {
                        if let Ok(entry) = serde_json::from_str::<InteractionEntry>(&line) {
                            if let Some(emb) = &entry.embedding {
                                let score = cosine_similarity(query_embedding, emb);
                                let doc_id = entry.ts.to_rfc3339();
                                dense_results.push((score, doc_id, entry));
                            }
                        }
                    }
                }
            }
        }
    }

    // Sort dense results and take top 50
    dense_results.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    dense_results.truncate(50);

    // Convert to ScoredHit format
    let dense_hits: Vec<ScoredHit> = dense_results
        .iter()
        .map(|(score, doc_id, entry)| ScoredHit {
            doc_id: doc_id.clone(),
            score: *score,
            source: HitSource::DenseInteraction,
            ts: Some(entry.ts),
        })
        .collect();

    // Perform RRF fusion with fallback for sparse dense results
    let mut fused = if dense_hits.len() < min_dense_hits() {
        log::debug!(
            "[Hybrid] Sparse dense results ({}), using BM25-only fallback",
            dense_hits.len()
        );
        fuse_rrf_multi(&[&bm25_hits], rrf_k_default(), limit)
    } else {
        fuse_rrf_multi(&[&bm25_hits, &dense_hits], rrf_k_default(), limit)
    };

    // Apply temporal boost for recency
    apply_temporal_boost(&mut fused, temporal_tau_days());

    // Map fused doc_ids back to InteractionEntry
    // Build lookup from doc_id -> entry
    let entry_map: std::collections::HashMap<String, InteractionEntry> = dense_results
        .into_iter()
        .map(|(_, doc_id, entry)| (doc_id, entry))
        .collect();

    // Also need to load entries for BM25-only results
    let mut final_results: Vec<InteractionEntry> = Vec::with_capacity(fused.len());
    for scored in fused {
        if let Some(entry) = entry_map.get(&scored.doc_id) {
            final_results.push(entry.clone());
        } else {
            // Entry was in BM25 but not in dense (no embedding) - load from JSONL
            if let Ok(entry) = find_entry_by_doc_id(app_handle, &scored.doc_id) {
                final_results.push(entry);
            }
        }
    }

    Ok(final_results)
}

/// Find an interaction entry by its doc_id (RFC3339 timestamp)
fn find_entry_by_doc_id<R: Runtime>(
    app_handle: &AppHandle<R>,
    doc_id: &str,
) -> Result<InteractionEntry, String> {
    let dir = get_interactions_dir(app_handle)?;

    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("jsonl") {
                if let Ok(file) = fs::File::open(&path) {
                    let reader = BufReader::new(file);
                    #[allow(clippy::lines_filter_map_ok)]
                    for line in reader.lines().filter_map(Result::ok) {
                        if let Ok(entry) = serde_json::from_str::<InteractionEntry>(&line) {
                            if entry.ts.to_rfc3339() == doc_id {
                                return Ok(entry);
                            }
                        }
                    }
                }
            }
        }
    }

    Err(format!("Entry not found: {}", doc_id))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_similarity_comprehensive() {
        // Test identical vectors
        let vec_query = vec![1.0, 2.0, 3.0];
        let vec_identical = vec![1.0, 2.0, 3.0];
        assert!((cosine_similarity(&vec_query, &vec_identical) - 1.0).abs() < 1e-6);

        // Test orthogonal vectors
        let vec_a = vec![1.0, 0.0, 0.0];
        let vec_orthogonal = vec![0.0, 1.0, 0.0];
        assert!((cosine_similarity(&vec_a, &vec_orthogonal) - 0.0).abs() < 1e-6);

        // Test opposite vectors
        let vec_opposite = vec![-1.0, 0.0, 0.0];
        assert!((cosine_similarity(&vec_a, &vec_opposite) - (-1.0)).abs() < 1e-6);

        // Test zero vector (should return 0.0, not NaN)
        let vec_zero = vec![0.0, 0.0, 0.0];
        assert_eq!(cosine_similarity(&vec_a, &vec_zero), 0.0);

        // Test different length vectors (should return 0.0 as per updated logic)
        let vec_short = vec![1.0, 2.0];
        let vec_long = vec![1.0, 2.0, 3.0];
        assert_eq!(cosine_similarity(&vec_short, &vec_long), 0.0);
    }
}
