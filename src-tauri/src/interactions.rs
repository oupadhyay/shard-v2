/**
 * Interactions module - Logs full conversation history and provides RAG retrieval
 *
 * Implements Tier 3 of the memory system:
 * - Logs every turn to daily JSONL files
 * - Generates embeddings using gemini-embedding-001
 * - Performs semantic search for context retrieval
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

#[derive(Serialize, Deserialize, Debug)]
struct EmbeddingRequest {
    content: EmbeddingContent,
    #[serde(rename = "outputDimensionality")]
    output_dimensionality: Option<u32>,
}

#[derive(Serialize, Deserialize, Debug)]
struct EmbeddingContent {
    parts: Vec<EmbeddingPart>,
}

#[derive(Serialize, Deserialize, Debug)]
struct EmbeddingPart {
    text: String,
}

#[derive(Deserialize, Debug)]
struct EmbeddingResponse {
    embedding: EmbeddingValues,
}

#[derive(Deserialize, Debug)]
struct EmbeddingValues {
    values: Vec<f32>,
}

// ============================================================================
// Embedding API
// ============================================================================

pub async fn generate_embedding(
    client: &reqwest::Client,
    text: &str,
    api_key: &str,
) -> Result<Vec<f32>, String> {
    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/gemini-embedding-001:embedContent?key={}",
        api_key
    );

    let payload = EmbeddingRequest {
        content: EmbeddingContent {
            parts: vec![EmbeddingPart {
                text: text.to_string(),
            }],
        },
        output_dimensionality: Some(768),
    };

    let res = client
        .post(&url)
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("Embedding API network error: {}", e))?;

    if !res.status().is_success() {
        let error_text = res.text().await.unwrap_or_default();
        return Err(format!("Embedding API error: {}", error_text));
    }

    let body: EmbeddingResponse = res
        .json()
        .await
        .map_err(|e| format!("Failed to parse embedding response: {}", e))?;

    Ok(body.embedding.values)
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

    writeln!(writer, "{}", json)
        .map_err(|e| format!("Failed to write interaction: {}", e))?;

    Ok(())
}

// ============================================================================
// RAG Retrieval
// ============================================================================

pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot_product: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }

    dot_product / (norm_a * norm_b)
}

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
                    for line in reader.lines().flatten() {
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
    Ok(results.into_iter().take(limit).map(|(_, entry)| entry).collect())
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_similarity() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0]; // Identical
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 1e-5);

        let c = vec![0.0, 1.0, 0.0]; // Orthogonal
        assert!((cosine_similarity(&a, &c) - 0.0).abs() < 1e-5);

        let d = vec![-1.0, 0.0, 0.0]; // Opposite
        assert!((cosine_similarity(&a, &d) - -1.0).abs() < 1e-5);
    }
}
