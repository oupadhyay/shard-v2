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

    fs::write(&path, content)
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
    let topics_dir = get_topics_dir(app_handle)?;
    let mut new_index = TopicIndex {
        topics: std::collections::HashMap::new(),
    };
    let mut count = 0;

    let entries = fs::read_dir(&topics_dir)
        .map_err(|e| format!("Failed to read topics dir: {}", e))?;

    for entry in entries.flatten() {
        let path = entry.path();

        // Skip index.json and non-.md files
        if path.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }

        if let Some(topic) = path.file_stem().and_then(|s| s.to_str()) {
            let content = fs::read_to_string(&path)
                .map_err(|e| format!("Failed to read {}: {}", topic, e))?;

            // Generate embedding
            let embedding_text = format!(
                "Topic: {}\nContent: {}",
                topic,
                content.chars().take(1000).collect::<String>()
            );
            let embedding =
                crate::interactions::generate_embedding(http_client, &embedding_text, api_key)
                    .await?;

            new_index.topics.insert(topic.to_string(), embedding);
            count += 1;
            log::info!("[Index] Rebuilt embedding for topic: {}", topic);
        }
    }

    save_topic_index(app_handle, &new_index)?;
    log::info!("[Index] Rebuilt index with {} topics", count);
    Ok(count)
}

/// Find relevant topic summaries based on query embedding (RAG)
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

/// Load memories from disk
pub fn load_memories<R: Runtime>(app_handle: &AppHandle<R>) -> Result<MemoryStore, String> {
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

/// Save memories to disk (both JSON and human-readable MD)
pub fn save_memories<R: Runtime>(app_handle: &AppHandle<R>, store: &MemoryStore) -> Result<(), String> {
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
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_creation() {
        let mem = Memory::new(
            MemoryCategory::Preference,
            "User prefers TypeScript".to_string(),
            3,
        );
        assert!(!mem.id.is_empty());
        assert_eq!(mem.importance, 3);
    }

    #[test]
    fn test_importance_clamping() {
        let mem_high = Memory::new(MemoryCategory::Fact, "test".to_string(), 10);
        assert_eq!(mem_high.importance, 5);

        let mem_low = Memory::new(MemoryCategory::Fact, "test".to_string(), 0);
        assert_eq!(mem_low.importance, 1);
    }

    #[test]
    fn test_memory_store_operations() {
        let mut store = MemoryStore::new();

        let mem = Memory::new(MemoryCategory::Preference, "Test memory".to_string(), 3);
        let id = mem.id.clone();
        store.add(mem);

        assert_eq!(store.memories.len(), 1);

        assert!(store.remove(&id));
        assert_eq!(store.memories.len(), 0);
    }

    #[test]
    fn test_token_budget_pruning() {
        let mut store = MemoryStore::new();

        // Add many low-importance memories
        for i in 0..10 {
            store.add(Memory::new(
                MemoryCategory::Fact,
                format!("This is a test memory number {} with some content to take up tokens", i),
                1,
            ));
        }

        // Add one high-importance memory
        store.add(Memory::new(
            MemoryCategory::Preference,
            "Important user preference".to_string(),
            5,
        ));

        // Prune to a small budget
        store.prune_to_token_budget(100);

        // High importance should survive
        assert!(store.memories.iter().any(|m| m.importance == 5));
    }

    #[test]
    fn test_format_for_prompt() {
        let mut store = MemoryStore::new();
        store.add(Memory::new(
            MemoryCategory::Preference,
            "User prefers Rust".to_string(),
            3,
        ));
        store.add(Memory::new(
            MemoryCategory::Project,
            "Working on shard-v2".to_string(),
            4,
        ));

        let formatted = store.format_for_prompt();
        assert!(formatted.contains("User prefers Rust"));
        assert!(formatted.contains("Working on shard-v2"));
        assert!(formatted.contains("### Preferences"));
        assert!(formatted.contains("### Project Context"));
    }
}
