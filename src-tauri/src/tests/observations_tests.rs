use crate::observations::*;
use crate::vector_store::VectorStore;
use tempfile::tempdir;

const EMBEDDING_DIM: usize = 768;

fn make_test_embedding(seed: f32) -> Vec<f32> {
    vec![seed; EMBEDDING_DIM]
}

fn open_test_store() -> (VectorStore, tempfile::TempDir) {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.sqlite");
    let store = VectorStore::open(&db_path).unwrap();
    (store, dir) // keep dir alive so the DB isn't deleted
}

// ============================================================================
// Observation CRUD
// ============================================================================

#[test]
fn test_insert_and_count() {
    let (store, _dir) = open_test_store();
    assert_eq!(count_observations(&store, "user").unwrap(), 0);

    let obs = make_observation("User lives in SF", ObservationLevel::Explicit, vec![], None);
    insert_observation(&store, &obs, None).unwrap();

    assert_eq!(count_observations(&store, "user").unwrap(), 1);
}

#[test]
fn test_insert_with_embedding() {
    let (store, _dir) = open_test_store();
    let obs = make_observation("User prefers Rust", ObservationLevel::Explicit, vec![], None);
    let emb = make_test_embedding(0.1);

    insert_observation(&store, &obs, Some(&emb)).unwrap();

    // Verify the embedding is searchable
    let results = search_observations_by_embedding(&store, "user", &emb, 5, 0.5).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].content, "User prefers Rust");
}

#[test]
fn test_insert_duplicate_is_ignored() {
    let (store, _dir) = open_test_store();
    let obs = make_observation("Fact A", ObservationLevel::Explicit, vec![], None);

    insert_observation(&store, &obs, None).unwrap();
    // Same ID again — INSERT OR IGNORE should silently skip
    insert_observation(&store, &obs, None).unwrap();

    assert_eq!(count_observations(&store, "user").unwrap(), 1);
}

#[test]
fn test_soft_delete() {
    let (store, _dir) = open_test_store();
    let obs = make_observation("Temporary fact", ObservationLevel::Explicit, vec![], None);
    let id = obs.id.clone();

    insert_observation(&store, &obs, None).unwrap();
    assert_eq!(count_observations(&store, "user").unwrap(), 1);

    soft_delete_observation(&store, &id).unwrap();
    // count_observations filters out deleted
    assert_eq!(count_observations(&store, "user").unwrap(), 0);
}

#[test]
fn test_source_ids_dag() {
    let (store, _dir) = open_test_store();

    // Create two explicit observations
    let obs_a = make_observation("User lives in SF", ObservationLevel::Explicit, vec![], None);
    let obs_b = make_observation("User works in tech", ObservationLevel::Explicit, vec![], None);
    insert_observation(&store, &obs_a, None).unwrap();
    insert_observation(&store, &obs_b, None).unwrap();

    // Create a deductive observation referencing both
    let obs_c = make_observation(
        "User likely commutes in Bay Area",
        ObservationLevel::Deductive,
        vec![obs_a.id.clone(), obs_b.id.clone()],
        None,
    );
    insert_observation(&store, &obs_c, None).unwrap();

    // Verify times_derived was incremented on parents
    let top = get_top_derived_observations(&store, "user", 10).unwrap();
    assert_eq!(top.len(), 2, "Both parent observations should have times_derived > 0");
    for obs in &top {
        assert_eq!(obs.times_derived, 1);
    }
}

// ============================================================================
// Retrieval
// ============================================================================

#[test]
fn test_get_by_level() {
    let (store, _dir) = open_test_store();

    insert_observation(&store, &make_observation("Fact 1", ObservationLevel::Explicit, vec![], None), None).unwrap();
    insert_observation(&store, &make_observation("Fact 2", ObservationLevel::Explicit, vec![], None), None).unwrap();
    insert_observation(&store, &make_observation("Pattern 1", ObservationLevel::Inductive, vec![], None), None).unwrap();

    let explicit = get_observations_by_level(&store, "user", ObservationLevel::Explicit, 10).unwrap();
    assert_eq!(explicit.len(), 2);

    let inductive = get_observations_by_level(&store, "user", ObservationLevel::Inductive, 10).unwrap();
    assert_eq!(inductive.len(), 1);
    assert_eq!(inductive[0].content, "Pattern 1");
}

#[test]
fn test_get_recent_observations() {
    let (store, _dir) = open_test_store();

    for i in 0..5 {
        let obs = make_observation(&format!("Fact {}", i), ObservationLevel::Explicit, vec![], None);
        insert_observation(&store, &obs, None).unwrap();
    }

    let recent = get_recent_observations(&store, "user", 3).unwrap();
    assert_eq!(recent.len(), 3);
}

#[test]
fn test_get_top_derived_empty() {
    let (store, _dir) = open_test_store();
    // Insert observation without any derivatives
    insert_observation(&store, &make_observation("Lonely fact", ObservationLevel::Explicit, vec![], None), None).unwrap();

    let top = get_top_derived_observations(&store, "user", 5).unwrap();
    assert!(top.is_empty(), "No observations have times_derived > 0");
}

#[test]
fn test_fts5_keyword_search() {
    let (store, _dir) = open_test_store();

    insert_observation(&store, &make_observation("User loves hiking in Yosemite", ObservationLevel::Explicit, vec![], None), None).unwrap();
    insert_observation(&store, &make_observation("User codes in Rust daily", ObservationLevel::Explicit, vec![], None), None).unwrap();
    insert_observation(&store, &make_observation("User likes coffee", ObservationLevel::Explicit, vec![], None), None).unwrap();

    let results = search_observations_by_keyword(&store, "user", "hiking Yosemite", 5).unwrap();
    assert!(!results.is_empty());
    assert!(results[0].content.contains("hiking"));
}

#[test]
fn test_fts5_empty_query() {
    let (store, _dir) = open_test_store();
    insert_observation(&store, &make_observation("Some fact", ObservationLevel::Explicit, vec![], None), None).unwrap();

    let results = search_observations_by_keyword(&store, "user", "", 5).unwrap();
    assert!(results.is_empty());
}

// ============================================================================
// Working Representation (blended search)
// ============================================================================

#[test]
fn test_working_representation_deduplicates() {
    let (store, _dir) = open_test_store();
    let emb = make_test_embedding(0.2);

    // Insert one observation — it will appear in semantic, top-derived (if referenced), and recent
    let obs = make_observation("User prefers dark mode", ObservationLevel::Explicit, vec![], None);
    insert_observation(&store, &obs, Some(&emb)).unwrap();

    let rep = get_working_representation(&store, "user", &emb, 10).unwrap();
    // Should have exactly 1 observation despite appearing in multiple buckets
    assert_eq!(rep.len(), 1);
    assert_eq!(rep[0].content, "User prefers dark mode");
}

#[test]
fn test_working_representation_empty_store() {
    let (store, _dir) = open_test_store();
    let emb = make_test_embedding(0.1);

    let rep = get_working_representation(&store, "user", &emb, 10).unwrap();
    assert!(rep.is_empty());
}

// ============================================================================
// Formatting
// ============================================================================

#[test]
fn test_format_observations_groups_by_level() {
    let observations = vec![
        Observation {
            id: "1".into(), observer: "shard".into(), observed: "user".into(),
            content: "Prefers functional style".into(),
            level: ObservationLevel::Inductive,
            source_ids: vec![], times_derived: 0, session_name: None,
            content_hash: "h1".into(), created_at: "2025-01-01".into(), deleted_at: None,
        },
        Observation {
            id: "2".into(), observer: "shard".into(), observed: "user".into(),
            content: "Works at a startup".into(),
            level: ObservationLevel::Explicit,
            source_ids: vec![], times_derived: 0, session_name: None,
            content_hash: "h2".into(), created_at: "2025-01-02".into(), deleted_at: None,
        },
        Observation {
            id: "3".into(), observer: "shard".into(), observed: "user".into(),
            content: "Likely interested in entrepreneurship".into(),
            level: ObservationLevel::Deductive,
            source_ids: vec!["2".into()], times_derived: 0, session_name: None,
            content_hash: "h3".into(), created_at: "2025-01-03".into(), deleted_at: None,
        },
    ];

    let md = format_observations_as_markdown(&observations);
    assert!(md.contains("**Patterns & Traits:**"));
    assert!(md.contains("Prefers functional style"));
    assert!(md.contains("**Known Facts:**"));
    assert!(md.contains("Works at a startup"));
    assert!(md.contains("**Inferred:**"));
    assert!(md.contains("Likely interested in entrepreneurship"));
}

#[test]
fn test_format_observations_empty() {
    let md = format_observations_as_markdown(&[]);
    assert!(md.is_empty());
}

// ============================================================================
// Peer Card
// ============================================================================

#[test]
fn test_peer_card_upsert_and_get() {
    let (store, _dir) = open_test_store();

    // Initially no card
    let card = get_peer_card(&store, "shard", "user").unwrap();
    assert!(card.is_none());

    // Upsert
    let facts = vec!["Lives in SF".into(), "Codes in Rust".into()];
    upsert_peer_card(&store, "shard", "user", &facts).unwrap();

    let card = get_peer_card(&store, "shard", "user").unwrap().unwrap();
    assert_eq!(card.facts.len(), 2);
    assert!(card.facts.contains(&"Lives in SF".to_string()));
    assert!(card.facts.contains(&"Codes in Rust".to_string()));
}

#[test]
fn test_peer_card_update_overwrites() {
    let (store, _dir) = open_test_store();

    upsert_peer_card(&store, "shard", "user", &["Fact A".into()]).unwrap();
    upsert_peer_card(&store, "shard", "user", &["Fact B".into(), "Fact C".into()]).unwrap();

    let card = get_peer_card(&store, "shard", "user").unwrap().unwrap();
    assert_eq!(card.facts.len(), 2);
    assert!(!card.facts.contains(&"Fact A".to_string()));
}

#[test]
fn test_format_peer_card() {
    let card = PeerCard {
        observer: "shard".into(),
        observed: "user".into(),
        facts: vec!["Lives in SF".into(), "Loves hiking".into()],
        updated_at: "2025-01-01".into(),
    };

    let md = format_peer_card(&card);
    assert!(md.contains("## User Card"));
    assert!(md.contains("- Lives in SF"));
    assert!(md.contains("- Loves hiking"));
}

#[test]
fn test_format_peer_card_empty() {
    let card = PeerCard {
        observer: "shard".into(),
        observed: "user".into(),
        facts: vec![],
        updated_at: "2025-01-01".into(),
    };

    let md = format_peer_card(&card);
    assert!(md.is_empty());
}

// ============================================================================
// make_observation helper
// ============================================================================

#[test]
fn test_make_observation_defaults() {
    let obs = make_observation("Hello", ObservationLevel::Explicit, vec![], Some("s1".into()));
    assert_eq!(obs.observer, "shard");
    assert_eq!(obs.observed, "user");
    assert_eq!(obs.level, ObservationLevel::Explicit);
    assert_eq!(obs.times_derived, 0);
    assert_eq!(obs.session_name, Some("s1".into()));
    assert!(!obs.id.is_empty());
    assert!(!obs.content_hash.is_empty());
    assert!(obs.deleted_at.is_none());
}

// ============================================================================
// ObservationLevel serde
// ============================================================================

#[test]
fn test_observation_level_roundtrip() {
    for level in &[ObservationLevel::Explicit, ObservationLevel::Deductive, ObservationLevel::Inductive, ObservationLevel::Contradiction] {
        let s = level.as_str();
        let parsed = ObservationLevel::parse_level(s).unwrap();
        assert_eq!(*level, parsed);
    }
}

#[test]
fn test_observation_level_invalid() {
    assert!(ObservationLevel::parse_level("bogus").is_none());
}
