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
