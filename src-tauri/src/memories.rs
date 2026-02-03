/**
 * Memories module - Persistent memory system for the AI agent
 *
 * Provides storage and retrieval of user preferences, project context,
 * and interaction summaries across chat sessions.
 */

use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::fs::{self};
use std::path::PathBuf;
use tauri::{AppHandle, Manager, Runtime};
use serde::{Deserialize, Serialize};

// ============================================================================
// Data Structures
// ============================================================================

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TopicIndex {
    pub topics: HashMap<String, Vec<f32>>, // topic_name -> embedding
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct InsightIndex {
    pub insights: HashMap<String, InsightMeta>, // title -> metadata
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct InsightMeta {
    pub embedding: Vec<f32>,
    pub reference_count: u32,  // Track access frequency
    pub update_count: u32,     // Track how many times information was added (for up-leveling)
    pub created_at: DateTime<Utc>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum MemoryCategory {
    Preference,    // User preferences (units, languages, coding style)
    Project,       // Project-specific context
    Interaction,   // Summarized past interactions
    Fact,          // General facts about the user
}

impl std::fmt::Display for MemoryCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MemoryCategory::Preference => write!(f, "preference"),
            MemoryCategory::Project => write!(f, "project"),
            MemoryCategory::Interaction => write!(f, "interaction"),
            MemoryCategory::Fact => write!(f, "fact"),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Memory {
    pub id: String,
    pub category: MemoryCategory,
    pub content: String,
    pub created_at: DateTime<Utc>,
    pub importance: u8, // 1-5
}

impl Memory {
    pub fn new(category: MemoryCategory, content: String, importance: u8) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            category,
            content,
            created_at: Utc::now(),
            importance: importance.clamp(1, 5),
        }
    }

    /// Estimate token count for this memory (rough: ~4 chars per token)
    pub fn estimated_tokens(&self) -> usize {
        (self.content.len() + 20) / 4  // +20 for category/formatting
    }
}

// ============================================================================
// Memory Store
// ============================================================================

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct MemoryStore {
    pub memories: Vec<Memory>,
    #[serde(default)]
    pub version: u32,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self {
            memories: Vec::new(),
            version: 1,
        }
    }

    /// Add a new memory to the store
    pub fn add(&mut self, memory: Memory) {
        self.memories.push(memory);
    }

    /// Remove a memory by ID
    pub fn remove(&mut self, id: &str) -> bool {
        let len_before = self.memories.len();
        self.memories.retain(|m| m.id != id);
        self.memories.len() < len_before
    }

    /// Get memories by category
    pub fn get_by_category(&self, category: &MemoryCategory) -> Vec<&Memory> {
        self.memories
            .iter()
            .filter(|m| &m.category == category)
            .collect()
    }

    /// Calculate total estimated tokens
    pub fn total_tokens(&self) -> usize {
        self.memories.iter().map(|m| m.estimated_tokens()).sum()
    }

    /// Prune to fit within token budget by removing lowest importance memories
    pub fn prune_to_token_budget(&mut self, max_tokens: usize) {
        if self.total_tokens() <= max_tokens {
            return;
        }

        // Sort by importance (ascending) so we remove lowest first
        self.memories.sort_by(|a, b| a.importance.cmp(&b.importance));

        while self.total_tokens() > max_tokens && !self.memories.is_empty() {
            self.memories.remove(0);
        }

        // Re-sort by created_at for consistent ordering
        self.memories.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    }

    /// Format memories as markdown for injection into system prompt
    pub fn format_for_prompt(&self) -> String {
        if self.memories.is_empty() {
            return String::new();
        }

        let mut output = String::from("\n## User Memories\n\n");

        // Group by category
        let categories = [
            (MemoryCategory::Preference, "Preferences"),
            (MemoryCategory::Project, "Project Context"),
            (MemoryCategory::Fact, "Facts"),
            (MemoryCategory::Interaction, "Past Interactions"),
        ];

        for (cat, header) in categories {
            let items: Vec<_> = self.get_by_category(&cat);
            if !items.is_empty() {
                output.push_str(&format!("### {}\n", header));
                for mem in items {
                    output.push_str(&format!("- {}\n", mem.content));
                }
                output.push('\n');
            }
        }

        output
    }
}

// ============================================================================
// File I/O
// ============================================================================

const MEMORIES_FILENAME: &str = "MEMORIES.json";
const MEMORIES_MD_FILENAME: &str = "MEMORIES.md";
const TOKEN_BUDGET: usize = 1000;

/// Get the path to the memories directory
pub fn get_memories_dir<R: Runtime>(app_handle: &AppHandle<R>) -> Result<PathBuf, String> {
    let app_data_dir = app_handle
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to get app data dir: {}", e))?;

    let memories_dir = app_data_dir.join("memories");

    if !memories_dir.exists() {
        fs::create_dir_all(&memories_dir)
            .map_err(|e| format!("Failed to create memories directory: {}", e))?;
    }

    Ok(memories_dir)
}

/// Get the path to the topics directory
pub fn get_topics_dir<R: Runtime>(app_handle: &AppHandle<R>) -> Result<PathBuf, String> {
    let memories_dir = get_memories_dir(app_handle)?;
    let topics_dir = memories_dir.join("topics");

    if !topics_dir.exists() {
        fs::create_dir_all(&topics_dir)
            .map_err(|e| format!("Failed to create topics directory: {}", e))?;
    }

    Ok(topics_dir)
}

fn get_topic_index_path<R: Runtime>(app_handle: &AppHandle<R>) -> Result<PathBuf, String> {
    let topics_dir = get_topics_dir(app_handle)?;
    Ok(topics_dir.join("index.json"))
}

fn load_topic_index<R: Runtime>(app_handle: &AppHandle<R>) -> Result<TopicIndex, String> {
    let path = get_topic_index_path(app_handle)?;
    if !path.exists() {
        return Ok(TopicIndex { topics: HashMap::new() });
    }
    let content = fs::read_to_string(&path)
        .map_err(|e| format!("Failed to read topic index: {}", e))?;
    serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse topic index: {}", e))
}

fn save_topic_index<R: Runtime>(app_handle: &AppHandle<R>, index: &TopicIndex) -> Result<(), String> {
    let path = get_topic_index_path(app_handle)?;
    let content = serde_json::to_string_pretty(index)
        .map_err(|e| format!("Failed to serialize topic index: {}", e))?;
    fs::write(&path, content)
        .map_err(|e| format!("Failed to write topic index: {}", e))
}

/// Read a focused topic summary
pub fn read_topic_summary<R: Runtime>(
    app_handle: &AppHandle<R>,
    topic: &str,
) -> Result<String, String> {
    let topics_dir = get_topics_dir(app_handle)?;
    // Sanitize filename
    let filename = format!("{}.md", topic.trim().replace(|c: char| !c.is_alphanumeric() && c != '_' && c != '-', "_"));
    let path = topics_dir.join(filename);

    if !path.exists() {
        return Err(format!("Topic summary not found: {}", topic));
    }

    fs::read_to_string(&path)
        .map_err(|e| format!("Failed to read topic summary: {}", e))
}

/// Update a focused topic summary (Async, generates embedding)
pub async fn update_topic_summary<R: Runtime>(
    app_handle: &AppHandle<R>,
    http_client: &reqwest::Client,
    api_key: &str,
    topic: &str,
    content: &str,
) -> Result<(), String> {
    let topics_dir = get_topics_dir(app_handle)?;
    // Sanitize filename
    let filename = format!("{}.md", topic.trim().replace(|c: char| !c.is_alphanumeric() && c != '_' && c != '-', "_"));
    let path = topics_dir.join(filename);

    fs::write(&path, format!("# {}\n\n{}", topic, content))
        .map_err(|e| format!("Failed to write topic summary: {}", e))?;

    // Generate embedding for the topic content (or just topic name + start of content)
    // We'll use the first 1000 chars of content to represent the topic semantically
    let embedding_text = format!("Topic: {}\nContent: {}", topic, content.chars().take(1000).collect::<String>());
    let embedding = crate::interactions::generate_embedding(http_client, &embedding_text, api_key).await?;

    // Update index
    let mut index = load_topic_index(app_handle)?;
    index.topics.insert(topic.to_string(), embedding);
    save_topic_index(app_handle, &index)?;

    log::info!("Topic summary updated: {}", topic);
    Ok(())
}

/// Rebuild the topic index from all existing .md files in topics directory
/// Call this after renaming/deleting topic files manually
pub async fn rebuild_topic_index<R: Runtime>(
    app_handle: &AppHandle<R>,
    http_client: &reqwest::Client,
    api_key: &str,
) -> Result<usize, String> {
    use futures::StreamExt;

    let topics_dir = get_topics_dir(app_handle)?;
    let mut new_index = TopicIndex {
        topics: std::collections::HashMap::new(),
    };

    let entries = fs::read_dir(&topics_dir)
        .map_err(|e| format!("Failed to read topics dir: {}", e))?;

    // Collect all topics to process
    let mut topics_to_process: Vec<(String, String)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();

        // Skip index.json and non-.md files
        if path.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }

        if let Some(topic) = path.file_stem().and_then(|s| s.to_str()) {
            if let Ok(content) = fs::read_to_string(&path) {
                let embedding_text = format!(
                    "Topic: {}\nContent: {}",
                    topic,
                    content.chars().take(1000).collect::<String>()
                );
                topics_to_process.push((topic.to_string(), embedding_text));
            }
        }
    }

    let count = topics_to_process.len();

    // Process embeddings concurrently with limit of 4 to avoid rate limiting
    let results: Vec<_> = futures::stream::iter(topics_to_process)
        .map(|(topic, text)| {
            let client = http_client.clone();
            let key = api_key.to_string();
            async move {
                match crate::interactions::generate_embedding(&client, &text, &key).await {
                    Ok(embedding) => {
                        log::info!("[Index] Rebuilt embedding for topic: {}", topic);
                        Some((topic, embedding))
                    }
                    Err(e) => {
                        log::error!("[Index] Failed to generate embedding for {}: {}", topic, e);
                        None
                    }
                }
            }
        })
        .buffer_unordered(4) // Concurrency limit
        .collect()
        .await;

    // Insert successful results into index
    for result in results.into_iter().flatten() {
        new_index.topics.insert(result.0, result.1);
    }

    save_topic_index(app_handle, &new_index)?;
    log::info!("[Index] Rebuilt index with {} topics", new_index.topics.len());
    Ok(count)
}

/// Find relevant topic summaries based on query embedding (RAG)
/// Note: Superseded by find_relevant_context() which handles both topics and insights
#[allow(dead_code)]
pub fn find_relevant_topics<R: Runtime>(
    app_handle: &AppHandle<R>,
    query_embedding: &[f32],
) -> Result<Option<(String, String)>, String> {
    let index = load_topic_index(app_handle)?;
    let mut best_score = -1.0;
    let mut best_topic = None;

    for (topic, embedding) in index.topics {
        let score = crate::interactions::cosine_similarity(query_embedding, &embedding);
        if score > best_score {
            best_score = score;
            best_topic = Some(topic);
        }
    }

    // Threshold? User said "first most semantically similar".
    // But if score is very low, maybe we shouldn't return anything?
    // Let's set a low threshold like 0.4 to avoid complete noise.
    if best_score > 0.4 {
        if let Some(topic) = best_topic {
            if let Ok(content) = read_topic_summary(app_handle, &topic) {
                return Ok(Some((topic, content)));
            }
        }
    }

    Ok(None)
}

// ============================================================================
// Insights (Tier 2.5) - Granular atomic facts for specific queries
// ============================================================================


/// Get the path to the insights directory
pub fn get_insights_dir<R: Runtime>(app_handle: &AppHandle<R>) -> Result<PathBuf, String> {
    let memories_dir = get_memories_dir(app_handle)?;
    let insights_dir = memories_dir.join("insights");

    if !insights_dir.exists() {
        fs::create_dir_all(&insights_dir)
            .map_err(|e| format!("Failed to create insights directory: {}", e))?;
    }

    Ok(insights_dir)
}

fn get_insight_index_path<R: Runtime>(app_handle: &AppHandle<R>) -> Result<PathBuf, String> {
    let insights_dir = get_insights_dir(app_handle)?;
    Ok(insights_dir.join("index.json"))
}

pub fn load_insight_index<R: Runtime>(app_handle: &AppHandle<R>) -> Result<InsightIndex, String> {
    let path = get_insight_index_path(app_handle)?;
    if !path.exists() {
        return Ok(InsightIndex::default());
    }
    let content = fs::read_to_string(&path)
        .map_err(|e| format!("Failed to read insight index: {}", e))?;
    serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse insight index: {}", e))
}

pub fn save_insight_index<R: Runtime>(app_handle: &AppHandle<R>, index: &InsightIndex) -> Result<(), String> {
    let path = get_insight_index_path(app_handle)?;
    let content = serde_json::to_string_pretty(index)
        .map_err(|e| format!("Failed to serialize insight index: {}", e))?;
    fs::write(&path, content)
        .map_err(|e| format!("Failed to write insight index: {}", e))
}

/// Sanitize a title to a valid filename
fn sanitize_filename(title: &str) -> String {
    title.trim().replace(|c: char| !c.is_alphanumeric() && c != '_' && c != '-', "_")
}

/// Read an insight file
pub fn read_insight<R: Runtime>(
    app_handle: &AppHandle<R>,
    title: &str,
) -> Result<String, String> {
    let insights_dir = get_insights_dir(app_handle)?;
    let filename = format!("{}.md", sanitize_filename(title));
    let path = insights_dir.join(filename);

    if !path.exists() {
        return Err(format!("Insight not found: {}", title));
    }

    fs::read_to_string(&path)
        .map_err(|e| format!("Failed to read insight: {}", e))
}

/// Create or update an insight (Async, generates embedding)
pub async fn update_insight<R: Runtime>(
    app_handle: &AppHandle<R>,
    http_client: &reqwest::Client,
    api_key: &str,
    title: &str,
    content: &str,
) -> Result<(), String> {
    let insights_dir = get_insights_dir(app_handle)?;
    let filename = format!("{}.md", sanitize_filename(title));
    let path = insights_dir.join(&filename);

    // Write markdown with heading format
    let formatted_content = format!("# {}\n\n{}", title, content);
    fs::write(&path, formatted_content)
        .map_err(|e| format!("Failed to write insight: {}", e))?;

    // Generate embedding
    let embedding_text = format!("Insight: {}\nContent: {}", title, content.chars().take(1000).collect::<String>());
    let embedding = crate::interactions::generate_embedding(http_client, &embedding_text, api_key).await?;

    // Update index (preserve counts if exists)
    let mut index = load_insight_index(app_handle)?;
    let (reference_count, update_count) = index.insights.get(title)
        .map(|m| (m.reference_count, m.update_count + 1))
        .unwrap_or((0, 1)); // Start at 1 for new insights

    index.insights.insert(title.to_string(), InsightMeta {
        embedding,
        reference_count,
        update_count,
        created_at: Utc::now(),
    });
    save_insight_index(app_handle, &index)?;

    log::info!("Insight updated: {}", title);
    Ok(())
}

/// Delete an insight file and remove from index
pub fn delete_insight<R: Runtime>(
    app_handle: &AppHandle<R>,
    title: &str,
) -> Result<bool, String> {
    let insights_dir = get_insights_dir(app_handle)?;
    let filename = format!("{}.md", sanitize_filename(title));
    let path = insights_dir.join(&filename);

    let file_deleted = if path.exists() {
        fs::remove_file(&path)
            .map_err(|e| format!("Failed to delete insight file: {}", e))?;
        true
    } else {
        false
    };

    // Remove from index
    let mut index = load_insight_index(app_handle)?;
    let was_in_index = index.insights.remove(title).is_some();
    if was_in_index {
        save_insight_index(app_handle, &index)?;
    }

    log::info!("Insight deleted: {}", title);
    Ok(file_deleted || was_in_index)
}

/// Increment reference count for an insight
pub fn increment_insight_reference<R: Runtime>(
    app_handle: &AppHandle<R>,
    title: &str,
) -> Result<u32, String> {
    let mut index = load_insight_index(app_handle)?;
    if let Some(meta) = index.insights.get_mut(title) {
        meta.reference_count += 1;
        let new_count = meta.reference_count;
        save_insight_index(app_handle, &index)?;
        Ok(new_count)
    } else {
        Err(format!("Insight not found in index: {}", title))
    }
}

/// Get insights that are candidates for promotion to topics (update_count >= threshold)
pub fn get_promotion_candidates<R: Runtime>(
    app_handle: &AppHandle<R>,
    threshold: u32,
) -> Result<Vec<String>, String> {
    let index = load_insight_index(app_handle)?;
    let candidates = index.insights.iter()
        .filter(|(_, meta)| meta.update_count >= threshold)
        .map(|(title, _)| title.clone())
        .collect();
    Ok(candidates)
}

/// Find relevant insights based on query embedding (RAG)
/// Returns highest-scoring insight if above threshold
pub fn find_relevant_insights<R: Runtime>(
    app_handle: &AppHandle<R>,
    query_embedding: &[f32],
) -> Result<Option<(String, String, f32)>, String> {
    let index = load_insight_index(app_handle)?;
    let mut best_score = -1.0f32;
    let mut best_title = None;

    for (title, meta) in index.insights.iter() {
        let score = crate::interactions::cosine_similarity(query_embedding, &meta.embedding);
        if score > best_score {
            best_score = score;
            best_title = Some(title.clone());
        }
    }

    // Same threshold as topics (0.4)
    if best_score > 0.4 {
        if let Some(title) = best_title {
            if let Ok(content) = read_insight(app_handle, &title) {
                return Ok(Some((title, content, best_score)));
            }
        }
    }

    Ok(None)
}

/// Find best match between topics and insights, preferring insights on tie
/// Returns (name, content, is_insight)
pub fn find_relevant_context<R: Runtime>(
    app_handle: &AppHandle<R>,
    query_embedding: &[f32],
) -> Result<Option<(String, String, bool)>, String> {
    let insight_result = find_relevant_insights(app_handle, query_embedding)?;

    // Get topic score for comparison (need to duplicate some logic)
    let topic_index = load_topic_index(app_handle)?;
    let mut topic_score = -1.0f32;
    let mut best_topic = None;
    for (topic, embedding) in topic_index.topics.iter() {
        let score = crate::interactions::cosine_similarity(query_embedding, embedding);
        if score > topic_score {
            topic_score = score;
            best_topic = Some(topic.clone());
        }
    }

    match insight_result {
        Some((title, content, insight_score)) => {
            // Prefer insight if score >= topic score (insight wins ties)
            if insight_score >= topic_score {
                // Increment reference count for this insight
                let _ = increment_insight_reference(app_handle, &title);
                Ok(Some((title, content, true)))
            } else if topic_score > 0.4 {
                if let Some(topic) = best_topic {
                    if let Ok(content) = read_topic_summary(app_handle, &topic) {
                        return Ok(Some((topic, content, false)));
                    }
                }
                Ok(None)
            } else {
                Ok(None)
            }
        }
        None => {
            // No insight match, try topics
            if topic_score > 0.4 {
                if let Some(topic) = best_topic {
                    if let Ok(content) = read_topic_summary(app_handle, &topic) {
                        return Ok(Some((topic, content, false)));
                    }
                }
            }
            Ok(None)
        }
    }
}

/// Rebuild the insight index by regenerating embeddings for all insight files
pub async fn rebuild_insight_index<R: Runtime>(
    app_handle: &AppHandle<R>,
    http_client: &reqwest::Client,
    api_key: &str,
) -> Result<usize, String> {
    let insights_dir = get_insights_dir(app_handle)?;
    if !insights_dir.exists() {
        return Ok(0);
    }

    let mut index = InsightIndex::default();
    let mut count = 0;

    if let Ok(entries) = fs::read_dir(&insights_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("md") {
                if let Some(title) = path.file_stem().and_then(|s| s.to_str()) {
                    if let Ok(content) = fs::read_to_string(&path) {
                        let embedding_text = format!("Insight: {}\nContent: {}", title, content.chars().take(1000).collect::<String>());
                        match crate::interactions::generate_embedding(http_client, &embedding_text, api_key).await {
                            Ok(embedding) => {
                                index.insights.insert(title.to_string(), InsightMeta {
                                    embedding,
                                    reference_count: 0,
                                    update_count: 1, // Assume 1 update for existing files
                                    created_at: Utc::now(),
                                });
                                count += 1;
                                log::info!("Indexed insight: {}", title);
                            }
                            Err(e) => {
                                log::error!("Failed to generate embedding for insight {}: {}", title, e);
                            }
                        }
                    }
                }
            }
        }
    }

    save_insight_index(app_handle, &index)?;
    Ok(count)
}

/// Load memories from disk (bypassing cache)
pub fn load_memories_from_disk<R: Runtime>(app_handle: &AppHandle<R>) -> Result<MemoryStore, String> {
    let memories_dir = get_memories_dir(app_handle)?;
    let json_path = memories_dir.join(MEMORIES_FILENAME);

    if !json_path.exists() {
        return Ok(MemoryStore::new());
    }

    let content = fs::read_to_string(&json_path)
        .map_err(|e| format!("Failed to read memories file: {}", e))?;

    serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse memories JSON: {}", e))
}

/// Load memories, using cache if available
pub fn load_memories<R: Runtime>(app_handle: &AppHandle<R>) -> Result<MemoryStore, String> {
    // Try to get from cache first
    let state = app_handle.state::<crate::AppState>();
    if let Ok(guard) = state.memory_store.read() {
        if let Some(store) = &*guard {
            return Ok(store.clone());
        }
    }

    // Fallback to disk if cache empty (should happen only once at startup)
    let store = load_memories_from_disk(app_handle)?;

    // Update cache
    if let Ok(mut guard) = state.memory_store.write() {
        *guard = Some(store.clone());
    }

    Ok(store)
}

/// Save memories to disk and update cache
pub fn save_memories<R: Runtime>(app_handle: &AppHandle<R>, store: &MemoryStore) -> Result<(), String> {
    // Update cache first
    let state = app_handle.state::<crate::AppState>();
    if let Ok(mut guard) = state.memory_store.write() {
        *guard = Some(store.clone());
    }

    let memories_dir = get_memories_dir(app_handle)?;

    // Save JSON (source of truth)
    let json_path = memories_dir.join(MEMORIES_FILENAME);
    let json_content = serde_json::to_string_pretty(store)
        .map_err(|e| format!("Failed to serialize memories: {}", e))?;

    fs::write(&json_path, json_content)
        .map_err(|e| format!("Failed to write memories JSON: {}", e))?;

    // Also write human-readable markdown
    let md_path = memories_dir.join(MEMORIES_MD_FILENAME);
    let md_content = format!(
        "# Agent Memories\n\n*Auto-generated from MEMORIES.json - edit that file for persistence*\n\n{}",
        store.format_for_prompt()
    );

    fs::write(&md_path, md_content)
        .map_err(|e| format!("Failed to write memories MD: {}", e))?;

    Ok(())
}

/// Add a memory and save to disk (enforces token budget)
pub fn add_memory<R: Runtime>(
    app_handle: &AppHandle<R>,
    category: MemoryCategory,
    content: String,
    importance: u8,
) -> Result<Memory, String> {
    // Load will check cache
    let mut store = load_memories(app_handle)?;

    let memory = Memory::new(category, content, importance);
    store.add(memory.clone());

    // Enforce token budget
    store.prune_to_token_budget(TOKEN_BUDGET);

    save_memories(app_handle, &store)?;

    log::info!("Memory saved: {} (importance: {})", memory.content, memory.importance);

    Ok(memory)
}

// TODO: Feature Request - Background cleanup job that runs daily to:
// 1. Remove stale/low-importance memories
// 2. Summarize old interaction memories
// 3. Consolidate duplicate preferences
/// Delete a memory by ID
#[allow(dead_code)]
pub fn delete_memory<R: Runtime>(app_handle: &AppHandle<R>, id: &str) -> Result<bool, String> {
    let mut store = load_memories(app_handle)?;
    let removed = store.remove(id);

    if removed {
        save_memories(app_handle, &store)?;
        log::info!("Memory deleted: {}", id);
    }

    Ok(removed)
}

/// Get formatted memories for prompt injection
pub fn get_memories_for_prompt<R: Runtime>(app_handle: &AppHandle<R>) -> Result<String, String> {
    let store = load_memories(app_handle)?;
    Ok(store.format_for_prompt())
}

// ============================================================================
// Daily Log (Clawdbot-style memory files)
// ============================================================================

/// Get the path to the daily memory log directory
pub fn get_memory_log_dir<R: Runtime>(app_handle: &AppHandle<R>) -> Result<PathBuf, String> {
    let memories_dir = get_memories_dir(app_handle)?;
    let log_dir = memories_dir.join("memory");

    if !log_dir.exists() {
        fs::create_dir_all(&log_dir)
            .map_err(|e| format!("Failed to create memory log directory: {}", e))?;
    }

    Ok(log_dir)
}

/// Get the path to today's daily log file
pub fn get_today_log_path<R: Runtime>(app_handle: &AppHandle<R>) -> Result<PathBuf, String> {
    let log_dir = get_memory_log_dir(app_handle)?;
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    Ok(log_dir.join(format!("{}.md", today)))
}

/// Append content to today's daily memory log
///
/// Used by pre-compaction flush to save extracted facts before summarization.
/// Creates the file with a header if it doesn't exist.
pub fn append_to_daily_log<R: Runtime>(
    app_handle: &AppHandle<R>,
    content: &str,
) -> Result<(), String> {
    use std::io::Write;

    let log_path = get_today_log_path(app_handle)?;

    // If file doesn't exist, create with header
    let needs_header = !log_path.exists();

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|e| format!("Failed to open daily log: {}", e))?;

    if needs_header {
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        writeln!(file, "# {}\n", today)
            .map_err(|e| format!("Failed to write header: {}", e))?;
    }

    write!(file, "{}", content)
        .map_err(|e| format!("Failed to append to daily log: {}", e))?;

    log::info!("[Memory] Appended to daily log: {}", log_path.display());
    Ok(())
}

/// Read all daily log files and return their contents
/// Returns Vec of (date, content) pairs, oldest first
pub fn read_all_daily_logs<R: Runtime>(app_handle: &AppHandle<R>) -> Result<Vec<(String, String)>, String> {
    let log_dir = get_memory_log_dir(app_handle)?;

    if !log_dir.exists() {
        return Ok(Vec::new());
    }

    let mut logs: Vec<(String, String)> = Vec::new();

    let entries = fs::read_dir(&log_dir)
        .map_err(|e| format!("Failed to read memory log dir: {}", e))?;

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().map(|e| e == "md").unwrap_or(false) {
            if let Some(filename) = path.file_stem().and_then(|s| s.to_str()) {
                // Skip today's log (still being written to)
                let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
                if filename == today {
                    continue;
                }

                if let Ok(content) = fs::read_to_string(&path) {
                    logs.push((filename.to_string(), content));
                }
            }
        }
    }

    // Sort by date (oldest first)
    logs.sort_by(|a, b| a.0.cmp(&b.0));

    Ok(logs)
}

/// Archive a processed daily log by moving to archived/ subdirectory
pub fn archive_daily_log<R: Runtime>(app_handle: &AppHandle<R>, date: &str) -> Result<(), String> {
    let log_dir = get_memory_log_dir(app_handle)?;
    let archived_dir = log_dir.join("archived");

    if !archived_dir.exists() {
        fs::create_dir_all(&archived_dir)
            .map_err(|e| format!("Failed to create archived dir: {}", e))?;
    }

    let src = log_dir.join(format!("{}.md", date));
    let dst = archived_dir.join(format!("{}.md", date));

    if src.exists() {
        fs::rename(&src, &dst)
            .map_err(|e| format!("Failed to archive daily log: {}", e))?;
        log::info!("[Memory] Archived daily log: {} -> archived/", date);
    }

    Ok(())
}

