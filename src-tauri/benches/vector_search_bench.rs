use criterion::{black_box, criterion_group, criterion_main, Criterion};
use rand::RngExt;
use shard_lib::memories::{Chunk, SourceType};
use shard_lib::vector_store::VectorStore;
use tempfile::tempdir;

fn generate_random_embedding(dim: usize) -> Vec<f32> {
    let mut rng = rand::rng();
    (0..dim).map(|_| rng.random::<f32>()).collect()
}

fn benchmark_search(c: &mut Criterion) {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("bench.sqlite");

    let store = VectorStore::open(&db_path).unwrap();

    // Insert 1000 chunks
    let count = 1000;
    let dim = 768;
    let mut chunks = Vec::new();

    println!("Generating {} random vectors...", count);

    // Pre-generate data
    for i in 0..count {
        let embedding = generate_random_embedding(dim);
        let source_type = if i % 2 == 0 {
            SourceType::Topic
        } else {
            SourceType::Session
        };
        chunks.push(Chunk {
            id: format!("bench::vector::{}", i),
            source_type, // Mixed Topic and Session memory chunks
            source_name: "bench".to_string(),
            heading: None,
            text: "bench content".to_string(),
            start_line: 0,
            end_line: 0,
            embedding,
        });
    }

    println!("Populating SQLite VectorStore...");
    // Populate DB
    for chunk in &chunks {
        store.upsert_chunk(chunk).unwrap();
    }

    let query = generate_random_embedding(dim);

    let mut group = c.benchmark_group("Vector Search");
    group.sample_size(10); // Lower sample size for heavy operations

    // Benchmark SQLite KNN
    group.bench_function("sqlite_vec_knn_1000", |b| {
        b.iter(|| store.knn_search(black_box(&query), 5, -100.0).unwrap())
    });

    // Benchmark Naive Linear Scan (Simulating old memory usage)
    // Note: This benchmark includes the cosine calculation + sorting cost
    // It does NOT include the JSON deserialization cost (which was arguably huge)
    // So this is a conservative lower bound for the old system.
    group.bench_function("linear_scan_1000", |b| {
        b.iter(|| {
            let mut scored: Vec<(f32, &Chunk)> = chunks
                .iter()
                .map(|c| {
                    // Cosine sim
                    let dot: f32 = c.embedding.iter().zip(&query).map(|(a, b)| a * b).sum();
                    let mag_a: f32 = c.embedding.iter().map(|a| a * a).sum::<f32>().sqrt();
                    let mag_b: f32 = query.iter().map(|b| b * b).sum::<f32>().sqrt();
                    (dot / (mag_a * mag_b), c)
                })
                .collect();
            // Sort top 5 desc
            scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
            let _top_5: Vec<&Chunk> = scored.into_iter().take(5).map(|(_, c)| c).collect();
        })
    });

    group.finish();
}

criterion_group!(benches, benchmark_search);
criterion_main!(benches);
