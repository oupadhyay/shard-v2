use crate::vector_store::{compute_content_hash, VectorStore};
/**
 * Memories module - Persistent memory system for the AI agent
 *
 * Provides storage and retrieval of user preferences, project context,
 * and interaction summaries across chat sessions.
 */
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self};
use std::path::PathBuf;
use tauri::{AppHandle, Manager, Runtime};

// ============================================================================
// Data Structures
// ============================================================================

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct TopicIndex {
    /// Topic names (file stems without .md extension)
    pub topics: std::collections::HashSet<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct InsightIndex {
    pub insights: HashMap<String, InsightMeta>, // title -> metadata
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct InsightMeta {
    /// Track how many times this insight has been retrieved for RAG context.
    /// Used as a proxy for "importance by utility".
    pub reference_count: u32,
    /// Track how many times this insight has been reinforced with new information.
    /// Trigger: update_count >= 3 makes it a candidate for promotion to a Topic.
    pub update_count: u32,
    pub created_at: DateTime<Utc>,
}

// ============================================================================
// Chunk Structures (for granular markdown-aware retrieval)
// ============================================================================

/// Source type for a chunk (topic or insight)
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum SourceType {
    Topic,
    Insight,
    Session,
}

/// A chunk of content from a topic or insight file
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Chunk {
    /// Unique ID: "{source_type}::{source_name}::{index}"
    pub id: String,
    /// Whether this came from a topic or insight
    pub source_type: SourceType,
    /// Name of the source file (without .md extension)
    pub source_name: String,
    /// Section heading (if chunk starts with one)
    pub heading: Option<String>,
    /// The chunk text content
    pub text: String,
    /// 1-indexed start line in source file
    pub start_line: u32,
    /// 1-indexed end line in source file
    pub end_line: u32,
    /// 768-dim embedding vector
    pub embedding: Vec<f32>,
}

/// Index of all chunks across topics and insights
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct ChunkIndex {
    pub chunks: Vec<Chunk>,
    pub last_rebuilt: Option<DateTime<Utc>>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum MemoryCategory {
    Preference,  // User preferences (units, languages, coding style)
    Project,     // Project-specific context
    Interaction, // Summarized past interactions
    Fact,        // General facts about the user
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
        (self.content.len() + 20) / 4 // +20 for category/formatting
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
        self.memories
            .sort_by(|a, b| a.importance.cmp(&b.importance));

        while self.total_tokens() > max_tokens && !self.memories.is_empty() {
            self.memories.remove(0);
        }

        // Re-sort by created_at for consistent ordering
        self.memories
            .sort_by(|a, b| a.created_at.cmp(&b.created_at));
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

/// Get a connection to the vector store
pub fn get_vector_store<R: Runtime>(app_handle: &AppHandle<R>) -> Result<VectorStore, String> {
    let memories_dir = get_memories_dir(app_handle)?;
    let db_path = memories_dir.join("memories.sqlite");
    VectorStore::open(&db_path).map_err(|e| format!("Failed to open vector store: {}", e))
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

/// Get the path to the memory transcripts directory
pub fn get_memory_transcripts_dir<R: Runtime>(app_handle: &AppHandle<R>) -> Result<PathBuf, String> {
    let memories_dir = get_memories_dir(app_handle)?;
    let transcripts_dir = memories_dir.join("sessions");

    if !transcripts_dir.exists() {
        fs::create_dir_all(&transcripts_dir)
            .map_err(|e| format!("Failed to create sessions directory: {}", e))?;
    }

    Ok(transcripts_dir)
}

fn get_topic_index_path<R: Runtime>(app_handle: &AppHandle<R>) -> Result<PathBuf, String> {
    let topics_dir = get_topics_dir(app_handle)?;
    Ok(topics_dir.join("index.json"))
}

fn load_topic_index<R: Runtime>(app_handle: &AppHandle<R>) -> Result<TopicIndex, String> {
    let path = get_topic_index_path(app_handle)?;
    if !path.exists() {
        return Ok(TopicIndex::default());
    }
    let content =
        fs::read_to_string(&path).map_err(|e| format!("Failed to read topic index: {}", e))?;
    match serde_json::from_str::<TopicIndex>(&content) {
        Ok(index) => Ok(index),
        Err(_) => {
            // Backward-compat path: try to interpret the file as the old format.
            // Old format could be either:
            //   1. { "topics": { "topic_a": [...], "topic_b": [...] } } (wrapped)
            //   2. { "topic_a": [...], "topic_b": [...] } (flat map)
            let old_format: Result<HashMap<String, serde_json::Value>, _> =
                serde_json::from_str(&content);

            match old_format {
                Ok(map) => {
                    // Check if this is the wrapped format { "topics": { ... } }
                    if map.len() == 1 && map.contains_key("topics") {
                        if let Some(serde_json::Value::Object(inner)) = map.get("topics") {
                            log::info!(
                                "[Memories] Migrated topic index from wrapped legacy format"
                            );
                            let topics = inner.keys().cloned().collect();
                            return Ok(TopicIndex { topics });
                        }
                    }
                    // Otherwise, treat top-level keys as topic names (flat format)
                    log::info!("[Memories] Migrated topic index from flat legacy format");
                    let topics = map.keys().cloned().collect();
                    Ok(TopicIndex { topics })
                }
                Err(_) => {
                    // If we cannot parse the index in either format, treat it as invalid
                    // and reset to an empty index, allowing the file to be rebuilt.
                    log::warn!("[Memories] Failed to parse topic index, resetting to default");
                    Ok(TopicIndex::default())
                }
            }
        }
    }
}

fn save_topic_index<R: Runtime>(
    app_handle: &AppHandle<R>,
    index: &TopicIndex,
) -> Result<(), String> {
    let path = get_topic_index_path(app_handle)?;
    let content = serde_json::to_string_pretty(index)
        .map_err(|e| format!("Failed to serialize topic index: {}", e))?;
    fs::write(&path, content).map_err(|e| format!("Failed to write topic index: {}", e))
}

/// Read a focused topic summary
pub fn read_topic_summary<R: Runtime>(
    app_handle: &AppHandle<R>,
    topic: &str,
) -> Result<String, String> {
    let topics_dir = get_topics_dir(app_handle)?;
    // Sanitize filename then validate the sanitized result
    let sanitized = topic
        .trim()
        .replace(|c: char| !c.is_alphanumeric() && c != '_' && c != '-', "_");
    if !crate::skills::is_safe_filename(&sanitized) {
        return Err("Invalid topic name: path traversal detected".to_string());
    }
    let filename = format!("{}.md", sanitized);
    let path = topics_dir.join(filename);

    if !path.exists() {
        return Err(format!("Topic summary not found: {}", topic));
    }

    fs::read_to_string(&path).map_err(|e| format!("Failed to read topic summary: {}", e))
}

/// Update a focused topic summary
pub fn update_topic_summary<R: Runtime>(
    app_handle: &AppHandle<R>,
    topic: &str,
    content: &str,
) -> Result<(), String> {
    let topics_dir = get_topics_dir(app_handle)?;
    // Sanitize filename then validate the sanitized result
    let sanitized = topic
        .trim()
        .replace(|c: char| !c.is_alphanumeric() && c != '_' && c != '-', "_");
    if !crate::skills::is_safe_filename(&sanitized) {
        return Err("Invalid topic name: path traversal detected".to_string());
    }
    let filename = format!("{}.md", sanitized);
    let path = topics_dir.join(filename);

    // Write markdown with heading format to ensure better chunking/recall
    let formatted_content = if !content.trim_start().starts_with("# ") {
        format!("# {}\n\n{}", topic, content)
    } else {
        content.to_string()
    };
    fs::write(&path, formatted_content).map_err(|e| format!("Failed to write topic: {}", e))?;

    // Update index (just track topic names)
    let mut index = load_topic_index(app_handle)?;
    index.topics.insert(topic.to_string());
    save_topic_index(app_handle, &index)?;

    log::info!("Topic summary updated: {}", topic);
    Ok(())
}

/// Rebuild the topic index from all existing .md files in topics directory
pub fn rebuild_topic_index<R: Runtime>(app_handle: &AppHandle<R>) -> Result<usize, String> {
    let topics_dir = get_topics_dir(app_handle)?;
    let mut new_index = TopicIndex::default();

    let entries =
        fs::read_dir(&topics_dir).map_err(|e| format!("Failed to read topics dir: {}", e))?;

    for entry in entries.flatten() {
        let path = entry.path();

        // Skip index.json and non-.md files
        if path.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }

        if let Some(topic) = path.file_stem().and_then(|s| s.to_str()) {
            new_index.topics.insert(topic.to_string());
        }
    }

    let count = new_index.topics.len();
    save_topic_index(app_handle, &new_index)?;
    log::info!("[Index] Rebuilt topic index with {} topics", count);
    Ok(count)
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
    let content =
        fs::read_to_string(&path).map_err(|e| format!("Failed to read insight index: {}", e))?;
    match serde_json::from_str::<InsightIndex>(&content) {
        Ok(index) => Ok(index),
        Err(_) => {
            // Fall back to a default index on parse errors (e.g., after schema changes)
            // or schema mismatch.
            log::warn!("[Memories] Failed to parse insight index, resetting to default");
            Ok(InsightIndex::default())
        }
    }
}

pub fn save_insight_index<R: Runtime>(
    app_handle: &AppHandle<R>,
    index: &InsightIndex,
) -> Result<(), String> {
    let path = get_insight_index_path(app_handle)?;
    let content = serde_json::to_string_pretty(index)
        .map_err(|e| format!("Failed to serialize insight index: {}", e))?;
    fs::write(&path, content).map_err(|e| format!("Failed to write insight index: {}", e))
}

/// Sanitize a title to a valid filename
fn sanitize_filename(title: &str) -> String {
    title
        .trim()
        .replace(|c: char| !c.is_alphanumeric() && c != '_' && c != '-', "_")
}

/// Read an insight file
pub fn read_insight<R: Runtime>(app_handle: &AppHandle<R>, title: &str) -> Result<String, String> {
    let insights_dir = get_insights_dir(app_handle)?;
    let filename = format!("{}.md", sanitize_filename(title));
    let path = insights_dir.join(filename);

    if !path.exists() {
        return Err(format!("Insight not found: {}", title));
    }

    fs::read_to_string(&path).map_err(|e| format!("Failed to read insight: {}", e))
}

/// Create or update an insight
pub fn update_insight<R: Runtime>(
    app_handle: &AppHandle<R>,
    title: &str,
    content: &str,
) -> Result<(), String> {
    let safe_title = sanitize_filename(title);
    if !crate::skills::is_safe_filename(&safe_title) {
        return Err("Invalid insight title: path traversal detected".to_string());
    }

    let insights_dir = get_insights_dir(app_handle)?;
    let filename = format!("{}.md", safe_title);
    let path = insights_dir.join(&filename);

    // Write markdown with heading format
    let formatted_content = format!("# {}\n\n{}", title, content);
    fs::write(&path, formatted_content).map_err(|e| format!("Failed to write insight: {}", e))?;

    // Update index (preserve counts if exists)
    let mut index = load_insight_index(app_handle)?;
    let (reference_count, update_count) = index
        .insights
        .get(title)
        .map(|m| (m.reference_count, m.update_count + 1))
        .unwrap_or((0, 1)); // Start at 1 for new insights

    index.insights.insert(
        title.to_string(),
        InsightMeta {
            reference_count,
            update_count,
            created_at: Utc::now(),
        },
    );
    save_insight_index(app_handle, &index)?;

    log::info!("Insight updated: {}", title);
    Ok(())
}

/// Delete an insight file and remove from index
pub fn delete_insight<R: Runtime>(app_handle: &AppHandle<R>, title: &str) -> Result<bool, String> {
    let safe_title = sanitize_filename(title);
    if !crate::skills::is_safe_filename(&safe_title) {
        return Err("Invalid insight title: path traversal detected".to_string());
    }

    let insights_dir = get_insights_dir(app_handle)?;
    let filename = format!("{}.md", safe_title);
    let path = insights_dir.join(&filename);

    let file_deleted = if path.exists() {
        fs::remove_file(&path).map_err(|e| format!("Failed to delete insight file: {}", e))?;
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
    let candidates = index
        .insights
        .iter()
        .filter(|(_, meta)| meta.update_count >= threshold)
        .map(|(title, _)| title.clone())
        .collect();
    Ok(candidates)
}

/// Find best match between topics and insights, preferring insights on tie
/// Returns (name, content, is_insight)
/// Uses hybrid search (Vector + FTS). Returns None if no chunks match.
/// Callers should ensure the chunk index is built; no fallback to whole-document search.
pub fn find_relevant_context<R: Runtime>(
    app_handle: &AppHandle<R>,
    query_text: &str,
    query_embedding: &[f32],
) -> Result<Option<(String, String, bool)>, String> {
    // Use hybrid search (Vector + FTS)
    if let Ok(chunks) = find_relevant_chunks(app_handle, query_text, query_embedding, 1) {
        if let Some(chunk) = chunks.first() {
            let is_insight = chunk.source_type == SourceType::Insight;
            log::debug!(
                "[Context] Chunk hit: {}::{} (lines {}-{})",
                chunk.source_name,
                chunk.heading.as_deref().unwrap_or("no-heading"),
                chunk.start_line,
                chunk.end_line
            );

            // Increment reference count for insights (for up-leveling tracking)
            if is_insight {
                if let Err(e) = increment_insight_reference(app_handle, &chunk.source_name) {
                    log::warn!("[Context] Failed to increment insight reference: {}", e);
                }
            }

            return Ok(Some((
                chunk.source_name.clone(),
                chunk.text.clone(),
                is_insight,
            )));
        }
    }

    // No chunk matches - return None
    // Caller should rebuild chunk index if getting no results
    Ok(None)
}

/// Rebuild the insight index from all existing .md files
pub fn rebuild_insight_index<R: Runtime>(app_handle: &AppHandle<R>) -> Result<usize, String> {
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
                    // Just track metadata
                    index.insights.insert(
                        title.to_string(),
                        InsightMeta {
                            reference_count: 0,
                            update_count: 1, // Assume 1 update for existing files
                            created_at: Utc::now(),
                        },
                    );
                    count += 1;
                    log::info!("[Index] Indexed insight metadata: {}", title);
                }
            }
        }
    }

    save_insight_index(app_handle, &index)?;
    log::info!("[Index] Rebuilt insight index with {} insights", count);
    Ok(count)
}

/// Load memories from disk (bypassing cache)
pub fn load_memories_from_disk<R: Runtime>(
    app_handle: &AppHandle<R>,
) -> Result<MemoryStore, String> {
    let memories_dir = get_memories_dir(app_handle)?;
    let json_path = memories_dir.join(MEMORIES_FILENAME);

    if !json_path.exists() {
        return Ok(MemoryStore::new());
    }

    let content = fs::read_to_string(&json_path)
        .map_err(|e| format!("Failed to read memories file: {}", e))?;

    serde_json::from_str(&content).map_err(|e| format!("Failed to parse memories JSON: {}", e))
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
pub fn save_memories<R: Runtime>(
    app_handle: &AppHandle<R>,
    store: &MemoryStore,
) -> Result<(), String> {
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

    fs::write(&md_path, md_content).map_err(|e| format!("Failed to write memories MD: {}", e))?;

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

    log::info!(
        "Memory saved: {} (importance: {})",
        memory.content,
        memory.importance
    );

    Ok(memory)
}

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
        writeln!(file, "# {}\n", today).map_err(|e| format!("Failed to write header: {}", e))?;
    }

    write!(file, "{}", content).map_err(|e| format!("Failed to append to daily log: {}", e))?;

    log::info!("[Memory] Appended to daily log: {}", log_path.display());
    Ok(())
}

/// Read all daily log files and return their contents
/// Returns Vec of (date, content) pairs, oldest first
pub fn read_all_daily_logs<R: Runtime>(
    app_handle: &AppHandle<R>,
) -> Result<Vec<(String, String)>, String> {
    let log_dir = get_memory_log_dir(app_handle)?;

    if !log_dir.exists() {
        return Ok(Vec::new());
    }

    let mut logs: Vec<(String, String)> = Vec::new();

    let entries =
        fs::read_dir(&log_dir).map_err(|e| format!("Failed to read memory log dir: {}", e))?;

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
        fs::rename(&src, &dst).map_err(|e| format!("Failed to archive daily log: {}", e))?;
        log::info!("[Memory] Archived daily log: {} -> archived/", date);
    }

    Ok(())
}

// ============================================================================
// Chunking Pipeline (markdown-aware splitting for granular retrieval)
// ============================================================================

/// Rough token estimation (~4 chars per token)
fn estimate_tokens(text: &str) -> usize {
    text.len() / 4
}

/// Intermediate chunk before embedding
#[derive(Debug, Clone)]
pub struct RawChunk {
    pub heading: Option<String>,
    pub text: String,
    pub start_line: u32,
    pub end_line: u32,
}

/// Split markdown content into chunks at header boundaries
/// Strategy:
/// 1. Split at markdown headings (^#{1,6}\s)
/// 2. If a section exceeds max_tokens, force-split at line boundaries (not sentences)
/// 3. Add overlap_tokens from previous chunk for context continuity
/// 4. Merge chunks < min_tokens with the next chunk
pub fn chunk_markdown(
    content: &str,
    max_tokens: usize,
    overlap_tokens: usize,
    min_tokens: usize,
) -> Vec<RawChunk> {
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return vec![];
    }

    let mut chunks: Vec<RawChunk> = Vec::new();
    let mut current_text = String::new();
    let mut current_heading: Option<String> = None;
    let mut start_line: u32 = 1;
    use std::sync::OnceLock;
    static HEADING_RE: OnceLock<regex::Regex> = OnceLock::new();
    let heading_regex = HEADING_RE.get_or_init(|| regex::Regex::new(r"^#{1,6}\s+(.+)$").unwrap());

    for (idx, line) in lines.iter().enumerate() {
        let line_num = (idx + 1) as u32;

        // Check if this line is a heading
        if let Some(caps) = heading_regex.captures(line) {
            // Flush current chunk if we have content
            if !current_text.trim().is_empty() {
                chunks.push(RawChunk {
                    heading: current_heading.clone(),
                    text: current_text.trim().to_string(),
                    start_line,
                    end_line: line_num - 1,
                });
                current_text = String::new();
            }

            // Start new chunk with this heading
            current_heading = Some(caps[1].to_string());
            start_line = line_num;
            current_text.push_str(line);
            current_text.push('\n');
        } else {
            current_text.push_str(line);
            current_text.push('\n');

            // Force split if we exceed max_tokens
            if estimate_tokens(&current_text) > max_tokens {
                chunks.push(RawChunk {
                    heading: current_heading.clone(),
                    text: current_text.trim().to_string(),
                    start_line,
                    end_line: line_num,
                });
                current_text = String::new();
                current_heading = None; // Mid-section split has no heading
                start_line = line_num + 1;
            }
        }
    }

    // Flush remaining content
    if !current_text.trim().is_empty() {
        chunks.push(RawChunk {
            heading: current_heading,
            text: current_text.trim().to_string(),
            start_line,
            end_line: lines.len() as u32,
        });
    }

    // Merge small chunks with next chunk
    let mut merged: Vec<RawChunk> = Vec::new();
    let mut pending: Option<RawChunk> = None;

    for chunk in chunks {
        if let Some(mut p) = pending.take() {
            if estimate_tokens(&p.text) < min_tokens {
                // Merge with current chunk
                p.text.push_str("\n\n");
                p.text.push_str(&chunk.text);
                p.end_line = chunk.end_line;
                if p.heading.is_none() && chunk.heading.is_some() {
                    p.heading = chunk.heading;
                } else if let (Some(ref ph), Some(ref ch)) = (&p.heading, &chunk.heading) {
                    if ph != ch {
                        p.heading = Some(format!("{} > {}", ph, ch));
                    }
                }
                pending = Some(p);
            } else {
                merged.push(p);
                pending = Some(chunk);
            }
        } else {
            pending = Some(chunk);
        }
    }
    if let Some(p) = pending {
        merged.push(p);
    }

    // Add overlap from previous chunk
    let mut final_chunks: Vec<RawChunk> = Vec::new();
    for (idx, chunk) in merged.iter().enumerate() {
        let mut text_with_overlap = String::new();

        if idx > 0 && overlap_tokens > 0 {
            // Get tail of previous chunk for overlap
            let prev_text = &merged[idx - 1].text;
            let overlap_chars = overlap_tokens * 4;
            if prev_text.len() > overlap_chars {
                let safe_start = prev_text
                    .char_indices()
                    .rev()
                    .nth(
                        overlap_chars
                            .min(prev_text.chars().count())
                            .saturating_sub(1),
                    )
                    .map(|(i, _)| i)
                    .unwrap_or(0);
                let overlap = &prev_text[safe_start..];
                text_with_overlap.push_str("...");
                text_with_overlap.push_str(overlap.trim_start());
                text_with_overlap.push_str("\n\n");
            }
        }

        text_with_overlap.push_str(&chunk.text);

        final_chunks.push(RawChunk {
            heading: chunk.heading.clone(),
            text: text_with_overlap,
            start_line: chunk.start_line,
            end_line: chunk.end_line,
        });
    }

    final_chunks
}

/// Rebuild the chunk index from all topic and insight files
/// Uses VectorStore with embedding cache for efficient updates
pub async fn rebuild_chunk_index<R: Runtime>(
    app_handle: &AppHandle<R>,
    http_client: &reqwest::Client,
    api_key: &str,
) -> Result<usize, String> {
    use futures::StreamExt;
    use std::collections::HashSet;

    // Get existing sources BEFORE any .await to avoid !Send issues with rusqlite::Connection.
    // VectorStore owns a Connection which is !Send, so we must not hold it across .await.
    let known_sources: HashSet<(SourceType, String)> = {
        let vector_store = get_vector_store(app_handle)?;
        let existing_sources = vector_store
            .get_unique_sources()
            .map_err(|e| format!("Failed to get sources: {}", e))?;
        HashSet::from_iter(existing_sources.into_iter())
    };
    // Note: vector_store is dropped here before any .await
    let mut processed_sources: HashSet<(SourceType, String)> = HashSet::new();

    let mut files_to_process: Vec<(SourceType, String, String)> = Vec::new();

    // 1. Scan Topics
    let topics_dir = get_topics_dir(app_handle)?;
    if topics_dir.exists() {
        if let Ok(entries) = fs::read_dir(&topics_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("md") {
                    if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
                        if let Ok(content) = fs::read_to_string(&path) {
                            files_to_process.push((SourceType::Topic, name.to_string(), content));
                            processed_sources.insert((SourceType::Topic, name.to_string()));
                        }
                    }
                }
            }
        }
    }

    // 2. Scan Insights
    let insights_dir = get_insights_dir(app_handle)?;
    if insights_dir.exists() {
        if let Ok(entries) = fs::read_dir(&insights_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("md") {
                    if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
                        if let Ok(content) = fs::read_to_string(&path) {
                            files_to_process.push((SourceType::Insight, name.to_string(), content));
                            processed_sources.insert((SourceType::Insight, name.to_string()));
                        }
                    }
                }
            }
        }
    }

    // 3. Scan Sessions
    let sessions_dir = get_memory_transcripts_dir(app_handle)?;
    if sessions_dir.exists() {
        if let Ok(entries) = fs::read_dir(&sessions_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("md") {
                    if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
                        if let Ok(content) = fs::read_to_string(&path) {
                            files_to_process.push((SourceType::Session, name.to_string(), content));
                            processed_sources.insert((SourceType::Session, name.to_string()));
                        }
                    }
                }
            }
        }
    }

    log::info!("[Chunk] Processing {} files", files_to_process.len());

    // 3. Process each file (concurrent embedding generation)
    // We create a stream of futures that process chunks

    let mut tasks = Vec::new();

    // Track sources and how many chunks we expect per source
    let mut sources_to_rebuild: Vec<(SourceType, String)> = Vec::new();
    let mut expected_chunks_per_source: std::collections::HashMap<(SourceType, String), usize> =
        std::collections::HashMap::new();

    for (source_type, source_name, content) in files_to_process {
        sources_to_rebuild.push((source_type.clone(), source_name.clone()));

        let raw_chunks = chunk_markdown(&content, 400, 80, 50);
        let chunk_count = raw_chunks.len();
        expected_chunks_per_source.insert((source_type.clone(), source_name.clone()), chunk_count);

        for (idx, raw) in raw_chunks.into_iter().enumerate() {
            let s_type = source_type.clone();
            let s_name = source_name.clone();

            tasks.push((s_type, s_name, idx, raw));
        }
    }

    // Step A: Check cache and prepare embedding tasks
    // Re-open vector store for cache lookups (dropped before .await)
    let mut chunks_to_embed = Vec::new();
    let mut ready_chunks = Vec::new();

    {
        let vector_store = get_vector_store(app_handle)?;
        for (s_type, s_name, idx, raw) in tasks {
            // Cache key: use sha256(chunk.text) to match what upsert_chunk stores
            // This ensures cache consistency between rebuild and upsert
            let content_hash = compute_content_hash(&raw.text);

            // Try cache
            let cached = match vector_store.get_cached_embedding(&content_hash) {
                Ok(emb) => emb,
                Err(e) => {
                    log::warn!("[Chunk] Cache check failed: {}", e);
                    None
                }
            };

            if let Some(embedding) = cached {
                let type_str = if s_type == SourceType::Topic {
                    "topic"
                } else {
                    "insight"
                };
                ready_chunks.push(Chunk {
                    id: format!("{}::{}::{}", type_str, s_name, idx),
                    source_type: s_type,
                    source_name: s_name,
                    heading: raw.heading,
                    text: raw.text,
                    start_line: raw.start_line,
                    end_line: raw.end_line,
                    embedding,
                });
            } else {
                chunks_to_embed.push((s_type, s_name, idx, raw));
            }
        }
    } // vector_store dropped here before .await

    log::info!(
        "[Chunk] Found {} chunks cached, {} to embed",
        ready_chunks.len(),
        chunks_to_embed.len()
    );

    // Step B: Generate embeddings in parallel
    let generated_chunks: Vec<Option<Chunk>> = futures::stream::iter(chunks_to_embed.into_iter())
        .map(|(s_type, s_name, idx, raw)| {
            let client = http_client.clone();
            let key = api_key.to_string();
            let embedding_text = format!(
                "{}: {}\n{}",
                if s_type == SourceType::Topic {
                    "Topic"
                } else {
                    "Insight"
                },
                s_name,
                &raw.text.chars().take(1000).collect::<String>()
            );

            async move {
                match crate::interactions::generate_embedding(&client, &embedding_text, &key).await
                {
                    Ok(embedding) => {
                        let type_str = if s_type == SourceType::Topic {
                            "topic"
                        } else {
                            "insight"
                        };
                        Some(Chunk {
                            id: format!("{}::{}::{}", type_str, s_name, idx),
                            source_type: s_type,
                            source_name: s_name,
                            heading: raw.heading,
                            text: raw.text,
                            start_line: raw.start_line,
                            end_line: raw.end_line,
                            embedding,
                        })
                    }
                    Err(e) => {
                        log::error!("[Chunk] Failed to embed {}::{}: {}", s_name, idx, e);
                        None
                    }
                }
            }
        })
        .buffer_unordered(4) // 4 concurrent requests
        .collect()
        .await;

    // Step D: Save everything to DB (offload blocking writes to thread pool)
    let gen_chunks: Vec<Chunk> = generated_chunks.into_iter().flatten().collect();
    let chunks_to_save = ready_chunks;
    let handle = app_handle.clone();
    let sources = sources_to_rebuild.clone();
    let known = known_sources.clone();
    let processed = processed_sources.clone();
    let expected_per_source = expected_chunks_per_source.clone();

    // Count actual chunks we have per source (cached + generated)
    let mut actual_chunks_per_source: std::collections::HashMap<(SourceType, String), usize> =
        std::collections::HashMap::new();
    for chunk in &chunks_to_save {
        *actual_chunks_per_source
            .entry((chunk.source_type.clone(), chunk.source_name.clone()))
            .or_insert(0) += 1;
    }
    for chunk in &gen_chunks {
        *actual_chunks_per_source
            .entry((chunk.source_type.clone(), chunk.source_name.clone()))
            .or_insert(0) += 1;
    }

    let saved_count =
        tokio::task::spawn_blocking(move || -> Result<usize, String> {
            let vs = get_vector_store(&handle)?;

            // Use with_transaction() for encapsulated, atomic writes.
            // Fail-fast: any upsert/delete error rolls back the entire transaction
            // to prevent a partially-committed index.
            vs.with_transaction(|vs, tx| {
                let mut count = 0;

                // Step C: Delete old chunks only for sources where ALL chunks are ready
                // Skip delete if any embedding failed for the source (to avoid data loss)
                for (s_type, s_name) in &sources {
                    let key = (s_type.clone(), s_name.clone());
                    let expected = expected_per_source.get(&key).copied().unwrap_or(0);
                    let actual = actual_chunks_per_source.get(&key).copied().unwrap_or(0);

                    if actual < expected {
                        log::warn!(
                            "[Chunk] Skipping delete for {} - only {}/{} chunks ready (embedding failures)",
                            s_name, actual, expected
                        );
                        continue;
                    }

                    vs.delete_by_source_internal(tx, s_type.clone(), s_name)?;
                }

                // Save cached chunks (fail-fast on any error)
                for chunk in &chunks_to_save {
                    vs.upsert_chunk_internal(tx, chunk)?;
                    count += 1;
                }

                // Save newly generated chunks (fail-fast on any error)
                for chunk in &gen_chunks {
                    vs.upsert_chunk_internal(tx, chunk)?;
                    count += 1;
                }

                // Cleanup deleted files
                for (s_type, s_name) in &known {
                    if !processed.contains(&(s_type.clone(), s_name.clone())) {
                        vs.delete_by_source_internal(tx, s_type.clone(), s_name)?;
                        log::info!("[Chunk] Removed deleted source: {}", s_name);
                    }
                }

                Ok(count)
            })
            .map_err(|e| format!("Transaction failed: {}", e))
        })
        .await
        .map_err(|e| format!("Blocking save task failed: {}", e))??;

    // Update metadata (re-open vector store since we're after .await)
    {
        let vector_store = get_vector_store(app_handle)?;
        if let Err(e) = vector_store.set_last_rebuilt(Utc::now()) {
            log::warn!("[Chunk] Failed to set last_rebuilt: {}", e);
        }
    }

    log::info!(
        "[Chunk] Index rebuilt locally: {} chunks active",
        saved_count
    );

    Ok(saved_count)
}

/// Find relevant chunks by embedding similarity and keyword match (Hybrid)
/// Uses VectorStore Hybrid search (sqlite-vec + FTS5)
pub fn find_relevant_chunks<R: Runtime>(
    app_handle: &AppHandle<R>,
    query_text: &str,
    query_embedding: &[f32],
    limit: usize,
) -> Result<Vec<Chunk>, String> {
    // Open vector store
    let vector_store = get_vector_store(app_handle)?;

    // Perform Hybrid search
    // Note: Vector part uses 0.35 threshold to filter low-quality semantic matches
    // while keeping FTS5 keyword matches for recall.
    let results = vector_store
        .hybrid_search(query_text, query_embedding, limit, 0.35)
        .map_err(|e| format!("Hybrid search failed: {}", e))?;

    Ok(results)
}

/// Search memory chunks with caller-specified min_score (for explicit memory_search tool).
/// Unlike find_relevant_chunks which hard-codes 0.35, this lets the model tune precision.
pub fn search_memory_chunks<R: Runtime>(
    app_handle: &AppHandle<R>,
    query_text: &str,
    query_embedding: &[f32],
    limit: usize,
    min_score: f32,
) -> Result<Vec<Chunk>, String> {
    let vector_store = get_vector_store(app_handle)?;
    let results = vector_store
        .hybrid_search(query_text, query_embedding, limit, min_score)
        .map_err(|e| format!("Hybrid search failed: {}", e))?;
    Ok(results)
}

/// Read specific lines from a memory file (topics, insights, or sessions).
/// `relative_path` is relative to the memories directory (e.g. "topics/SHARD.md").
/// Returns the requested line range as a string.
pub fn read_memory_file_lines<R: Runtime>(
    app_handle: &AppHandle<R>,
    relative_path: &str,
    from_line: usize,
    line_count: usize,
) -> Result<String, String> {
    let memories_dir = get_memories_dir(app_handle)?;
    let requested = memories_dir.join(relative_path);

    // Security: canonicalize and ensure the resolved path is inside memories_dir
    let canonical_memories = memories_dir
        .canonicalize()
        .map_err(|e| format!("Failed to canonicalize memories dir: {}", e))?;
    let canonical_requested = requested
        .canonicalize()
        .map_err(|_| format!("File not found: {}", relative_path))?;

    if !canonical_requested.starts_with(&canonical_memories) {
        return Err("Path traversal denied: path must be within the memories directory".to_string());
    }

    let content = fs::read_to_string(&canonical_requested)
        .map_err(|e| format!("Failed to read file: {}", e))?;

    let lines: Vec<&str> = content.lines().collect();
    let total_lines = lines.len();

    // Clamp from_line to valid range (1-indexed input)
    let start_idx = from_line.saturating_sub(1).min(total_lines);
    let end_idx = (start_idx + line_count).min(total_lines);

    let selected: Vec<&str> = lines[start_idx..end_idx].to_vec();
    let header = format!(
        "[Lines {}-{} of {} total in {}]\n",
        start_idx + 1,
        end_idx,
        total_lines,
        relative_path
    );

    Ok(format!("{}{}", header, selected.join("\n")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_topic_index_migration() {
        let legacy_json = r#"{"Topic A": {"metadata": {}}, "Topic B": {"metadata": {}}}"#;

        // Test the migration logic extracted into a parse attempt
        let old_format: Result<HashMap<String, serde_json::Value>, _> =
            serde_json::from_str(legacy_json);
        assert!(old_format.is_ok());

        let map = old_format.unwrap();
        let index = TopicIndex {
            topics: map.keys().cloned().collect(),
        };

        assert!(index.topics.contains(&"Topic A".to_string()));
        assert!(index.topics.contains(&"Topic B".to_string()));
        assert_eq!(index.topics.len(), 2);
    }
}
