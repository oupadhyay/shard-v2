/**
 * Retrieval module - BM25 and hybrid retrieval with Reciprocal Rank Fusion
 *
 * Implements:
 * - BM25 inverted index for lexical/keyword matching
 * - Reciprocal Rank Fusion (RRF) to combine BM25 + dense retrieval
 * - Hybrid search combining both modalities
 */

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use tauri::{AppHandle, Manager, Runtime};


// ============================================================================
// Data Structures
// ============================================================================

/// BM25 inverted index for lexical retrieval
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct BM25Index {
    /// Inverted index: term -> [(doc_id, term_frequency)]
    pub inverted_index: HashMap<String, Vec<(String, u32)>>,
    /// Document lengths (in tokens)
    pub doc_lengths: HashMap<String, u32>,
    /// Total token count across all documents (for avg calculation)
    pub total_tokens: u64,
    /// Total document count
    pub doc_count: u32,
}

/// Source of a retrieval hit (for debugging and fusion weighting)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitSource {
    Bm25,
    DenseInteraction,
    DenseTopicChunk, // future-proofing for chunked topic retrieval
}

/// A scored retrieval hit with metadata for fusion
#[derive(Debug, Clone)]
pub struct ScoredHit {
    pub doc_id: String,
    pub score: f32,
    pub source: HitSource,
    pub ts: Option<chrono::DateTime<chrono::Utc>>,
}

/// Legacy scored document (kept for backwards compatibility)
#[derive(Debug, Clone)]
pub struct ScoredDocument {
    pub doc_id: String,
    pub score: f32,
}

// ============================================================================
// Constants
// ============================================================================

/// Term frequency saturation parameter (BM25)
const BM25_K1: f32 = 1.2;
/// Length normalization parameter (BM25)
const BM25_B: f32 = 0.75;
/// RRF dampening constant (standard default)
const RRF_K_DEFAULT: f32 = 60.0;
/// Minimum dense hits before falling back to BM25-only
const MIN_DENSE_HITS: usize = 3;
/// Default temporal decay half-life in days
const TEMPORAL_TAU_DAYS: f32 = 15.0;

// ============================================================================
// Tokenization
// ============================================================================

/// Simple tokenizer: lowercase, split on whitespace and punctuation
///
/// TODO: Future improvements to consider:
/// - Use `unicode-segmentation` crate for proper word boundaries
/// - Add stopword removal (common words like "the", "is", "a")
/// - Handle code tokens specially (preserve `snake_case`, `camelCase`)
/// - Consider stemming with `rust-stemmers` crate
/// - Benchmark performance impact before adding complexity
pub fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty() && s.len() > 1) // Skip single chars
        .map(|s| s.to_string())
        .collect()
}

// ============================================================================
// BM25 Index Implementation
// ============================================================================

impl BM25Index {
    pub fn new() -> Self {
        Self::default()
    }

    /// Average document length
    pub fn avg_doc_length(&self) -> f32 {
        if self.doc_count == 0 {
            return 0.0;
        }
        self.total_tokens as f32 / self.doc_count as f32
    }

    /// Add a document to the index
    pub fn add_document(&mut self, doc_id: &str, content: &str) {
        let tokens = tokenize(content);
        let doc_length = tokens.len() as u32;

        // If document already exists, remove it first
        if self.doc_lengths.contains_key(doc_id) {
            self.remove_document(doc_id);
        }

        // Count term frequencies
        let mut term_freqs: HashMap<String, u32> = HashMap::new();
        for token in &tokens {
            *term_freqs.entry(token.clone()).or_insert(0) += 1;
        }

        // Update inverted index
        for (term, freq) in term_freqs {
            self.inverted_index
                .entry(term)
                .or_insert_with(Vec::new)
                .push((doc_id.to_string(), freq));
        }

        // Update document stats
        self.doc_lengths.insert(doc_id.to_string(), doc_length);
        self.total_tokens += doc_length as u64;
        self.doc_count += 1;
    }

    /// Remove a document from the index
    pub fn remove_document(&mut self, doc_id: &str) {
        if let Some(doc_length) = self.doc_lengths.remove(doc_id) {
            self.total_tokens = self.total_tokens.saturating_sub(doc_length as u64);
            self.doc_count = self.doc_count.saturating_sub(1);

            // Remove from inverted index
            for postings in self.inverted_index.values_mut() {
                postings.retain(|(id, _)| id != doc_id);
            }

            // Clean up empty terms
            self.inverted_index.retain(|_, v| !v.is_empty());
        }
    }

    /// Compute IDF for a term
    fn idf(&self, term: &str) -> f32 {
        let n = self.doc_count as f32;
        let df = self
            .inverted_index
            .get(term)
            .map(|v| v.len() as f32)
            .unwrap_or(0.0);

        if df == 0.0 {
            return 0.0;
        }

        // IDF formula: log((N - n(t) + 0.5) / (n(t) + 0.5) + 1)
        ((n - df + 0.5) / (df + 0.5) + 1.0).ln()
    }

    /// Search the index with BM25 scoring
    pub fn search(&self, query: &str, limit: usize) -> Vec<ScoredDocument> {
        let query_tokens = tokenize(query);
        if query_tokens.is_empty() {
            return Vec::new();
        }

        let avg_dl = self.avg_doc_length();
        let mut scores: HashMap<String, f32> = HashMap::new();

        for token in &query_tokens {
            let idf = self.idf(token);
            if idf == 0.0 {
                continue;
            }

            if let Some(postings) = self.inverted_index.get(token) {
                for (doc_id, tf) in postings {
                    let doc_length = *self.doc_lengths.get(doc_id).unwrap_or(&1) as f32;
                    let tf_f = *tf as f32;

                    // BM25 scoring formula
                    let numerator = tf_f * (BM25_K1 + 1.0);
                    let denominator = tf_f + BM25_K1 * (1.0 - BM25_B + BM25_B * doc_length / avg_dl);
                    let score = idf * numerator / denominator;

                    *scores.entry(doc_id.clone()).or_insert(0.0) += score;
                }
            }
        }

        // Sort by score descending
        let mut results: Vec<ScoredDocument> = scores
            .into_iter()
            .map(|(doc_id, score)| ScoredDocument { doc_id, score })
            .collect();

        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(limit);
        results
    }
}

// ============================================================================
// Reciprocal Rank Fusion
// ============================================================================

/// Compute RRF fusion of two ranked lists (legacy API, kept for compatibility)
///
/// RRF(d) = Σ 1/(k + rank_L(d))
/// where k is a dampening constant (default 60)
pub fn compute_rrf(
    bm25_results: &[ScoredDocument],
    dense_results: &[ScoredDocument],
    limit: usize,
) -> Vec<ScoredDocument> {
    let mut rrf_scores: HashMap<String, f32> = HashMap::new();

    // Add BM25 contributions (1-indexed ranks)
    for (rank, doc) in bm25_results.iter().enumerate() {
        let rrf_contribution = 1.0 / (RRF_K_DEFAULT + (rank + 1) as f32);
        *rrf_scores.entry(doc.doc_id.clone()).or_insert(0.0) += rrf_contribution;
    }

    // Add dense contributions (1-indexed ranks)
    for (rank, doc) in dense_results.iter().enumerate() {
        let rrf_contribution = 1.0 / (RRF_K_DEFAULT + (rank + 1) as f32);
        *rrf_scores.entry(doc.doc_id.clone()).or_insert(0.0) += rrf_contribution;
    }

    // Sort by RRF score descending
    let mut results: Vec<ScoredDocument> = rrf_scores
        .into_iter()
        .map(|(doc_id, score)| ScoredDocument { doc_id, score })
        .collect();

    results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    results.truncate(limit);
    results
}

/// Compute RRF fusion over N ranked lists of ScoredHit
///
/// RRF(d) = Σ_L 1/(k + rank_L(d))
/// Returns fused results sorted by RRF score
pub fn fuse_rrf_multi(lists: &[&[ScoredHit]], k: f32, limit: usize) -> Vec<ScoredHit> {
    use std::collections::HashMap;

    let mut rrf_scores: HashMap<String, f32> = HashMap::new();
    let mut hit_metadata: HashMap<String, (HitSource, Option<chrono::DateTime<chrono::Utc>>)> =
        HashMap::new();

    for list in lists {
        for (rank, hit) in list.iter().enumerate() {
            let rrf_contribution = 1.0 / (k + (rank + 1) as f32);
            *rrf_scores.entry(hit.doc_id.clone()).or_insert(0.0) += rrf_contribution;

            // Keep first source we encounter (arbitrary but consistent)
            hit_metadata
                .entry(hit.doc_id.clone())
                .or_insert((hit.source, hit.ts));
        }
    }

    // Sort by RRF score descending
    let mut results: Vec<ScoredHit> = rrf_scores
        .into_iter()
        .map(|(doc_id, score)| {
            let (source, ts) = hit_metadata.get(&doc_id).cloned().unwrap_or((HitSource::Bm25, None));
            ScoredHit {
                doc_id,
                score,
                source,
                ts,
            }
        })
        .collect();

    results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    results.truncate(limit);
    results
}

/// Apply exponential decay boost based on recency
///
/// boost = base_score * exp(-(now - ts) / τ)
/// where τ is half-life in days (default 15.0)
///
/// Hits without timestamps are left unchanged.
pub fn apply_temporal_boost(hits: &mut [ScoredHit], tau_days: f32) {
    let now = chrono::Utc::now();
    let tau_secs = tau_days * 24.0 * 3600.0;

    for hit in hits.iter_mut() {
        if let Some(ts) = hit.ts {
            let age_secs = (now - ts).num_seconds().max(0) as f32;
            let decay = (-age_secs / tau_secs).exp();
            hit.score *= decay;
        }
    }

    // Re-sort after boosting
    hits.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
}

/// Get the default minimum dense hits threshold (for external use)
pub fn min_dense_hits() -> usize {
    MIN_DENSE_HITS
}

/// Get the default temporal decay half-life in days (for external use)
pub fn temporal_tau_days() -> f32 {
    TEMPORAL_TAU_DAYS
}

/// Get the default RRF k constant (for external use)
pub fn rrf_k_default() -> f32 {
    RRF_K_DEFAULT
}

// ============================================================================
// Index Persistence
// ============================================================================

const BM25_INDEX_FILENAME: &str = "bm25_index.json";

fn get_bm25_index_path<R: Runtime>(app_handle: &AppHandle<R>) -> Result<PathBuf, String> {
    let app_data_dir = app_handle
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to get app data dir: {}", e))?;

    let interactions_dir = app_data_dir.join("interactions");
    if !interactions_dir.exists() {
        fs::create_dir_all(&interactions_dir)
            .map_err(|e| format!("Failed to create interactions dir: {}", e))?;
    }

    Ok(interactions_dir.join(BM25_INDEX_FILENAME))
}

/// Load BM25 index from disk with graceful fallback
pub fn load_bm25_index<R: Runtime>(app_handle: &AppHandle<R>) -> Result<BM25Index, String> {
    let path = get_bm25_index_path(app_handle)?;

    if !path.exists() {
        return Ok(BM25Index::new());
    }

    match fs::read_to_string(&path) {
        Ok(content) => match serde_json::from_str(&content) {
            Ok(index) => Ok(index),
            Err(e) => {
                log::warn!("BM25 index corrupted, starting fresh: {}", e);
                Ok(BM25Index::new())
            }
        },
        Err(e) => {
            log::warn!("Failed to read BM25 index, starting fresh: {}", e);
            Ok(BM25Index::new())
        }
    }
}

/// Save BM25 index to disk
pub fn save_bm25_index<R: Runtime>(
    app_handle: &AppHandle<R>,
    index: &BM25Index,
) -> Result<(), String> {
    let path = get_bm25_index_path(app_handle)?;
    let content = serde_json::to_string(index)
        .map_err(|e| format!("Failed to serialize BM25 index: {}", e))?;

    fs::write(&path, content).map_err(|e| format!("Failed to write BM25 index: {}", e))
}

/// Rebuild BM25 index from all JSONL interaction files
pub fn rebuild_bm25_index<R: Runtime>(app_handle: &AppHandle<R>) -> Result<usize, String> {
    let app_data_dir = app_handle
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to get app data dir: {}", e))?;

    let interactions_dir = app_data_dir.join("interactions");
    if !interactions_dir.exists() {
        return Ok(0);
    }

    let mut index = BM25Index::new();
    let mut count = 0;

    let entries = fs::read_dir(&interactions_dir)
        .map_err(|e| format!("Failed to read interactions dir: {}", e))?;

    for entry in entries.flatten() {
        let path = entry.path();

        // Only process .jsonl files
        if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
            continue;
        }

        if let Ok(content) = fs::read_to_string(&path) {
            for line in content.lines() {
                if let Ok(entry) = serde_json::from_str::<crate::interactions::InteractionEntry>(line) {
                    // Use timestamp as doc_id for uniqueness
                    let doc_id = entry.ts.to_rfc3339();
                    index.add_document(&doc_id, &entry.content);
                    count += 1;
                }
            }
        }
    }

    save_bm25_index(app_handle, &index)?;
    log::info!("[BM25] Rebuilt index with {} documents", count);

    Ok(count)
}

/// Prune old entries from BM25 index (called by background cleanup)
pub fn prune_bm25_index<R: Runtime>(
    app_handle: &AppHandle<R>,
    max_age_days: i64,
    max_docs: usize,
) -> Result<usize, String> {
    let mut index = load_bm25_index(app_handle)?;
    let initial_count = index.doc_count as usize;

    // Parse doc_ids as timestamps and remove old ones
    let cutoff = chrono::Utc::now() - chrono::Duration::days(max_age_days);
    let mut to_remove: Vec<String> = Vec::new();

    for doc_id in index.doc_lengths.keys() {
        if let Ok(ts) = chrono::DateTime::parse_from_rfc3339(doc_id) {
            if ts < cutoff {
                to_remove.push(doc_id.clone());
            }
        }
    }

    for doc_id in &to_remove {
        index.remove_document(doc_id);
    }

    // If still over max_docs, remove oldest
    if index.doc_count as usize > max_docs {
        let mut doc_ids: Vec<_> = index.doc_lengths.keys().cloned().collect();
        doc_ids.sort(); // RFC3339 timestamps sort chronologically

        let to_trim = index.doc_count as usize - max_docs;
        for doc_id in doc_ids.into_iter().take(to_trim) {
            index.remove_document(&doc_id);
        }
    }

    let removed = initial_count - index.doc_count as usize;
    if removed > 0 {
        save_bm25_index(app_handle, &index)?;
        log::info!("[BM25] Pruned {} old entries from index", removed);
    }

    Ok(removed)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tokenize() {
        let tokens = tokenize("Hello, World! This is a TEST.");
        assert!(tokens.contains(&"hello".to_string()));
        assert!(tokens.contains(&"world".to_string()));
        assert!(tokens.contains(&"test".to_string()));
        // Single chars filtered out
        assert!(!tokens.contains(&"a".to_string()));
    }

    #[test]
    fn test_tokenize_code() {
        let tokens = tokenize("fn main() { println!(\"hello\"); }");
        assert!(tokens.contains(&"fn".to_string()));
        assert!(tokens.contains(&"main".to_string()));
        assert!(tokens.contains(&"println".to_string()));
        assert!(tokens.contains(&"hello".to_string()));
    }

    #[test]
    fn test_bm25_add_document() {
        let mut index = BM25Index::new();
        index.add_document("doc1", "the quick brown fox");

        assert_eq!(index.doc_count, 1);
        assert!(index.inverted_index.contains_key("quick"));
        assert!(index.inverted_index.contains_key("brown"));
        assert!(index.inverted_index.contains_key("fox"));
        // "the" is filtered (single-use common word behavior varies)
    }

    #[test]
    fn test_bm25_remove_document() {
        let mut index = BM25Index::new();
        index.add_document("doc1", "hello world");
        index.add_document("doc2", "goodbye world");

        assert_eq!(index.doc_count, 2);

        index.remove_document("doc1");
        assert_eq!(index.doc_count, 1);
        assert!(!index.doc_lengths.contains_key("doc1"));
    }

    #[test]
    fn test_bm25_search_exact_match() {
        let mut index = BM25Index::new();
        index.add_document("doc1", "rust programming language");
        index.add_document("doc2", "python programming language");
        index.add_document("doc3", "javascript framework");

        let results = index.search("rust programming", 10);
        assert!(!results.is_empty());
        assert_eq!(results[0].doc_id, "doc1");
    }

    #[test]
    fn test_bm25_search_partial_match() {
        let mut index = BM25Index::new();
        index.add_document("doc1", "machine learning with neural networks");
        index.add_document("doc2", "deep learning algorithms");
        index.add_document("doc3", "cooking recipes");

        let results = index.search("learning", 10);
        assert_eq!(results.len(), 2);
        // Both doc1 and doc2 should be returned
        let doc_ids: Vec<_> = results.iter().map(|r| r.doc_id.clone()).collect();
        assert!(doc_ids.contains(&"doc1".to_string()));
        assert!(doc_ids.contains(&"doc2".to_string()));
    }

    #[test]
    fn test_rrf_fusion() {
        let bm25_results = vec![
            ScoredDocument { doc_id: "A".to_string(), score: 10.0 },
            ScoredDocument { doc_id: "B".to_string(), score: 8.0 },
            ScoredDocument { doc_id: "C".to_string(), score: 5.0 },
        ];

        let dense_results = vec![
            ScoredDocument { doc_id: "B".to_string(), score: 0.9 },
            ScoredDocument { doc_id: "D".to_string(), score: 0.8 },
            ScoredDocument { doc_id: "A".to_string(), score: 0.7 },
        ];

        let fused = compute_rrf(&bm25_results, &dense_results, 10);

        // B should be ranked highest (appears high in both lists)
        // A and B have contributions from both, D and C from one each
        assert!(!fused.is_empty());

        // Find B's position - should be at or near top
        let b_pos = fused.iter().position(|r| r.doc_id == "B");
        assert!(b_pos.is_some());
        assert!(b_pos.unwrap() <= 1); // B should be in top 2
    }

    #[test]
    fn test_rrf_single_list() {
        let bm25_results = vec![
            ScoredDocument { doc_id: "A".to_string(), score: 10.0 },
            ScoredDocument { doc_id: "B".to_string(), score: 8.0 },
        ];

        let dense_results: Vec<ScoredDocument> = vec![];

        let fused = compute_rrf(&bm25_results, &dense_results, 10);
        assert_eq!(fused.len(), 2);
        // Order should be preserved from BM25
        assert_eq!(fused[0].doc_id, "A");
        assert_eq!(fused[1].doc_id, "B");
    }

    #[test]
    fn test_fuse_rrf_multi_two_lists() {
        let now = chrono::Utc::now();
        let bm25_hits = vec![
            ScoredHit { doc_id: "A".to_string(), score: 10.0, source: HitSource::Bm25, ts: Some(now) },
            ScoredHit { doc_id: "B".to_string(), score: 8.0, source: HitSource::Bm25, ts: Some(now) },
        ];
        let dense_hits = vec![
            ScoredHit { doc_id: "B".to_string(), score: 0.9, source: HitSource::DenseInteraction, ts: Some(now) },
            ScoredHit { doc_id: "C".to_string(), score: 0.8, source: HitSource::DenseInteraction, ts: Some(now) },
        ];

        let fused = fuse_rrf_multi(&[&bm25_hits, &dense_hits], 60.0, 10);

        // B appears in both lists, should be ranked highest or near top
        let b_pos = fused.iter().position(|r| r.doc_id == "B");
        assert!(b_pos.is_some());
        assert!(b_pos.unwrap() <= 1);
    }

    #[test]
    fn test_fuse_rrf_multi_single_list() {
        let now = chrono::Utc::now();
        let hits = vec![
            ScoredHit { doc_id: "X".to_string(), score: 5.0, source: HitSource::Bm25, ts: Some(now) },
            ScoredHit { doc_id: "Y".to_string(), score: 3.0, source: HitSource::Bm25, ts: Some(now) },
        ];

        let fused = fuse_rrf_multi(&[&hits], 60.0, 10);
        assert_eq!(fused.len(), 2);
        assert_eq!(fused[0].doc_id, "X");
        assert_eq!(fused[1].doc_id, "Y");
    }

    #[test]
    fn test_fuse_rrf_multi_three_lists() {
        let now = chrono::Utc::now();
        let list1 = vec![ScoredHit { doc_id: "A".to_string(), score: 1.0, source: HitSource::Bm25, ts: Some(now) }];
        let list2 = vec![ScoredHit { doc_id: "A".to_string(), score: 1.0, source: HitSource::DenseInteraction, ts: Some(now) }];
        let list3 = vec![ScoredHit { doc_id: "A".to_string(), score: 1.0, source: HitSource::DenseTopicChunk, ts: Some(now) }];

        let fused = fuse_rrf_multi(&[&list1, &list2, &list3], 60.0, 10);

        // A appears in all 3 lists, should get highest RRF
        assert_eq!(fused.len(), 1);
        assert_eq!(fused[0].doc_id, "A");
        // RRF contribution = 3 * (1/(60+1)) ≈ 0.049
        assert!(fused[0].score > 0.04);
    }

    #[test]
    fn test_temporal_boost_recent_first() {
        let now = chrono::Utc::now();
        let hour_ago = now - chrono::Duration::hours(1);
        let month_ago = now - chrono::Duration::days(30);

        let mut hits = vec![
            ScoredHit { doc_id: "old".to_string(), score: 1.0, source: HitSource::Bm25, ts: Some(month_ago) },
            ScoredHit { doc_id: "new".to_string(), score: 1.0, source: HitSource::Bm25, ts: Some(hour_ago) },
        ];

        apply_temporal_boost(&mut hits, 15.0); // 15-day half-life

        // Recent hit should now be first due to higher decay-adjusted score
        assert_eq!(hits[0].doc_id, "new");
        assert!(hits[0].score > hits[1].score);
    }

    #[test]
    fn test_temporal_boost_no_timestamp() {
        let now = chrono::Utc::now();
        let mut hits = vec![
            ScoredHit { doc_id: "with_ts".to_string(), score: 1.0, source: HitSource::Bm25, ts: Some(now) },
            ScoredHit { doc_id: "no_ts".to_string(), score: 1.0, source: HitSource::Bm25, ts: None },
        ];

        apply_temporal_boost(&mut hits, 15.0);

        // Both should still be present, no timestamp hit retains score
        let no_ts = hits.iter().find(|h| h.doc_id == "no_ts").unwrap();
        assert!((no_ts.score - 1.0).abs() < 0.01); // Unchanged
    }
}
