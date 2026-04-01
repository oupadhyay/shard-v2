use shard_lib::observations::*;
use shard_lib::vector_store::VectorStore;
use tempfile::tempdir;
use std::time::Instant;

fn main() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("perf.sqlite");
    let store = VectorStore::open(&db_path).unwrap();

    println!("Populating database with 2000 observations...");
    let mut ids = Vec::new();
    for i in 0..2000 {
        let obs = make_observation(&format!("Fact {}", i), ObservationLevel::Explicit, vec![], None);
        ids.push(obs.id.clone());
        insert_observation(&store, &obs, None).unwrap();
    }

    println!("Creating 500 derivations...");
    for i in 0..500 {
        let source_ids = vec![ids[i].clone()];
        let derived = make_observation("Derived fact", ObservationLevel::Deductive, source_ids, None);
        insert_observation(&store, &derived, None).unwrap();
    }

    println!("--- Benchmarking get_top_derived_observations (limit 100) ---");
    let start = Instant::now();
    for _ in 0..100 {
        let _ = get_top_derived_observations(&store, "user", 100).unwrap();
    }
    println!("Avg time: {:?} per call", start.elapsed() / 100);

    println!("--- Benchmarking get_recent_observations (limit 100) ---");
    let start = Instant::now();
    for _ in 0..100 {
        let _ = get_recent_observations(&store, "user", 100).unwrap();
    }
    println!("Avg time: {:?} per call", start.elapsed() / 100);

    println!("--- Benchmarking search_observations_by_embedding (N+1 scenario) ---");
    let query_emb = vec![0.1f32; 768];
    // We need to insert an embedding to make search work
    let obs = make_observation("Embedded fact", ObservationLevel::Explicit, vec![], None);
    insert_observation(&store, &obs, Some(&query_emb)).unwrap();

    let start = Instant::now();
    for _ in 0..50 {
        let _ = search_observations_by_embedding(&store, "user", &query_emb, 10, 0.5).unwrap();
    }
    println!("Avg time: {:?} per call", start.elapsed() / 50);
}
