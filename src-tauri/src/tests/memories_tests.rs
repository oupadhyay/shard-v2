/**
 * Memory system tests
 */
use crate::memories::{Memory, MemoryCategory, MemoryStore};

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

// ============================================================================
// Chunking Algorithm Tests
// ============================================================================

use crate::memories::chunk_markdown;

#[test]
fn test_chunk_markdown_splits_at_headers() {
    let content = r#"# Section One

This is content in section one.
Some more lines here.

## Section Two

This is content in section two.
And another line.

### Section Three

Final section content.
"#;

    // Use large max_tokens to only split on headers
    let chunks = chunk_markdown(content, 10000, 0, 0);

    assert_eq!(chunks.len(), 3, "Should have 3 chunks for 3 headers");

    assert_eq!(chunks[0].heading, Some("Section One".to_string()));
    assert!(chunks[0].text.contains("content in section one"));
    assert_eq!(chunks[0].start_line, 1);

    assert_eq!(chunks[1].heading, Some("Section Two".to_string()));
    assert!(chunks[1].text.contains("content in section two"));

    assert_eq!(chunks[2].heading, Some("Section Three".to_string()));
    assert!(chunks[2].text.contains("Final section content"));
}

#[test]
fn test_chunk_markdown_respects_token_limit() {
    // Create content that will exceed token limit within one section
    // Use multiple lines since splitting happens line-by-line
    let mut lines = Vec::new();
    for i in 0..50 {
        lines.push(format!("This is line number {} with enough content to contribute to token count.", i));
    }
    let content = format!("# Long Section\n\n{}", lines.join("\n"));

    // Use small max_tokens to force splitting (100 tokens = ~400 chars)
    let chunks = chunk_markdown(&content, 100, 0, 0);

    // Should be split into multiple chunks
    assert!(chunks.len() > 1, "Long section should be split into multiple chunks, got {} chunks", chunks.len());

    // First chunk should have the heading
    assert_eq!(chunks[0].heading, Some("Long Section".to_string()));
}

#[test]
fn test_chunk_markdown_adds_overlap() {
    // Need a longer first section so overlap chars threshold is met
    // Overlap is 20 tokens = ~80 chars, so first section needs > 80 chars
    let content = r#"# First Section

This is the first section with a substantial amount of content that will be used for overlap context. It needs to be long enough to exceed the overlap character threshold of approximately eighty characters.

## Second Section

Second section content here.
"#;

    // Request 20 tokens of overlap (~80 chars)
    let chunks = chunk_markdown(content, 10000, 20, 0);

    assert_eq!(chunks.len(), 2);

    // First chunk should not have overlap prefix
    assert!(!chunks[0].text.starts_with("..."));

    // Second chunk should have overlap from first chunk
    assert!(chunks[1].text.starts_with("..."), "Second chunk should start with overlap marker, got: {}", &chunks[1].text[..50.min(chunks[1].text.len())]);
}

#[test]
fn test_chunk_markdown_merges_small_chunks() {
    let content = r#"# A

tiny

## B

also tiny

## C

More substantial content that definitely has more than 50 tokens worth of text in it to test the merging behavior properly.
"#;

    // min_tokens = 50, so small sections should merge
    let chunks = chunk_markdown(content, 10000, 0, 50);

    // Sections A and B are small and should be merged
    assert!(chunks.len() < 3, "Small chunks should be merged: got {} chunks", chunks.len());
}

#[test]
fn test_chunk_markdown_preserves_line_numbers() {
    let content = "# Section A\n\nLine 3\nLine 4\n\n## Section B\n\nLine 8\nLine 9\n";

    let chunks = chunk_markdown(content, 10000, 0, 0);

    assert_eq!(chunks.len(), 2, "Expected 2 chunks, got {}", chunks.len());

    // Section A starts at line 1 (the header)
    assert_eq!(chunks[0].start_line, 1);
    // Section A ends before Section B starts
    assert!(chunks[0].end_line < chunks[1].start_line, "Chunk 0 end ({}) should be < chunk 1 start ({})", chunks[0].end_line, chunks[1].start_line);

    // Section B starts at line 6 (after the empty line 5)
    assert_eq!(chunks[1].start_line, 6, "Section B should start at line 6, got {}", chunks[1].start_line);
}

#[test]
fn test_chunk_markdown_empty_content() {
    let chunks = chunk_markdown("", 400, 80, 50);
    assert!(chunks.is_empty());
}

#[test]
fn test_chunk_markdown_no_headers() {
    let content = "Just some plain text\nwithout any headers.\nMultiple lines.";

    let chunks = chunk_markdown(content, 10000, 0, 0);

    assert_eq!(chunks.len(), 1);
    assert!(chunks[0].heading.is_none());
    assert!(chunks[0].text.contains("plain text"));
    assert_eq!(chunks[0].start_line, 1);
    assert_eq!(chunks[0].end_line, 3);
}

// ============================================================================
// UTF-8 Safety & Heading Merge Tests
// ============================================================================

#[test]
fn test_chunk_markdown_overlap_utf8_safety() {
    let content = "# Section One\n\nThis is a long section with café résumé naïve characters and 日本語テスト Japanese text. We need enough content here to exceed the overlap character threshold which is overlap_tokens times four characters. Adding more filler text to make sure this section is definitely long enough for the overlap to trigger properly.\n\n## Section Two\n\nSecond section content.\n";

    let chunks = chunk_markdown(content, 10000, 20, 0);
    assert_eq!(chunks.len(), 2);
    assert!(chunks[1].text.starts_with("..."), "Overlap should be applied with multi-byte content");
}

#[test]
fn test_chunk_markdown_merge_preserves_specific_heading() {
    let content = "# Intro\n\ntiny\n\n## Detailed Topic\n\nThis section has much more detailed content that should be well above the minimum token threshold for merging.\n";

    let chunks = chunk_markdown(content, 10000, 0, 200);
    assert_eq!(chunks.len(), 1, "Both small chunks should merge");
    let heading = chunks[0].heading.as_deref().unwrap();
    assert!(heading.contains("Intro"), "Merged heading should contain first heading");
    assert!(heading.contains("Detailed Topic"), "Merged heading should contain second heading");
}

#[test]
fn test_chunk_markdown_merge_adopts_heading_when_pending_has_none() {
    let content = "Some intro text without a heading.\n\n## Named Section\n\nContent for named section.\n";

    let chunks = chunk_markdown(content, 10000, 0, 200);
    assert_eq!(chunks.len(), 1, "Should merge into one chunk");
    assert_eq!(chunks[0].heading, Some("Named Section".to_string()));
}

// ============================================================================
// Phase 2-3 Structural Tests
// ============================================================================

use crate::memories::{TopicIndex, InsightIndex, InsightMeta};
use chrono::Utc;

#[test]
fn test_topic_index_metadata_only_serialization() {
    let mut index = TopicIndex::default();
    index.topics.insert("Rust_Optimization".to_string());
    index.topics.insert("Tauri_Architecture".to_string());

    let serialized = serde_json::to_string(&index).expect("Failed to serialize TopicIndex");

    // Verify it doesn't contain "embedding" or large float arrays
    assert!(!serialized.contains("embedding"));

    let deserialized: TopicIndex = serde_json::from_str(&serialized).expect("Failed to deserialize TopicIndex");
    assert_eq!(deserialized.topics.len(), 2);
    assert!(deserialized.topics.contains("Rust_Optimization"));
}

#[test]
fn test_insight_meta_no_embedding_serialization() {
    let meta = InsightMeta {
        reference_count: 5,
        update_count: 2,
        created_at: Utc::now(),
    };

    let serialized = serde_json::to_string(&meta).expect("Failed to serialize InsightMeta");

    // Verify no embedding field
    assert!(!serialized.contains("embedding"));
    assert!(serialized.contains("\"reference_count\":5"));

    let deserialized: InsightMeta = serde_json::from_str(&serialized).expect("Failed to deserialize InsightMeta");
    assert_eq!(deserialized.reference_count, 5);
    assert_eq!(deserialized.update_count, 2);
}

#[test]
fn test_insight_index_metadata_only_serialization() {
    let mut index = InsightIndex::default();
    index.insights.insert("Fact_1".to_string(), InsightMeta {
        reference_count: 1,
        update_count: 1,
        created_at: Utc::now(),
    });

    let serialized = serde_json::to_string(&index).expect("Failed to serialize InsightIndex");
    assert!(!serialized.contains("embedding"));

    let deserialized: InsightIndex = serde_json::from_str(&serialized).expect("Failed to deserialize InsightIndex");
    assert!(deserialized.insights.contains_key("Fact_1"));
    assert_eq!(deserialized.insights.get("Fact_1").unwrap().reference_count, 1);
}

// ============================================================================
// Backward Compatibility Tests
// ============================================================================

#[test]
fn test_topic_index_old_format_deserializes_to_default() {
    let old_format = r#"{"topics":{"rust_optimization":[0.1,0.2,0.3],"tauri_architecture":[0.4,0.5,0.6]}}"#;
    let result: Result<TopicIndex, _> = serde_json::from_str(old_format);
    assert!(result.is_err(), "Old embedding-based format should not parse as new TopicIndex");
}

#[test]
fn test_insight_index_old_format_with_embedding_field() {
    let old_format = r#"{"insights":{"fact_1":{"embedding":[0.1,0.2],"reference_count":3,"update_count":1,"created_at":"2025-01-01T00:00:00Z"}}}"#;
    let result: Result<InsightIndex, _> = serde_json::from_str(old_format);
    assert!(result.is_ok(), "InsightMeta with extra 'embedding' field should still parse (serde default ignores unknown fields) OR fail gracefully");
}
