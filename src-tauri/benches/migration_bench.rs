use criterion::{black_box, criterion_group, criterion_main, Criterion};
use rand::Rng;
use shard_lib::memories::{Chunk, ChunkIndex, SourceType};
use shard_lib::vector_store::VectorStore;
use tempfile::tempdir;

fn generate_random_embedding(dim: usize) -> Vec<f32> {
    let mut rng = rand::thread_rng();
    (0..dim).map(|_| rng.gen::<f32>()).collect()
}

fn benchmark_migration(c: &mut Criterion) {
    // Pre-generate 1000 chunks
    let count = 1000;
    let dim = 768;
    let mut chunks = Vec::new();

    for i in 0..count {
        let embedding = generate_random_embedding(dim);
        chunks.push(Chunk {
            id: format!("bench::migration::{}", i),
            source_type: SourceType::Topic,
            source_name: "bench".to_string(),
            heading: Some(format!("Heading {}", i)),
            text: format!(
                "This is bench content for chunk {}. It needs some length to be realistic.",
                i
            ),
            start_line: i as u32,
            end_line: (i + 1) as u32,
            embedding,
        });
    }

    let chunk_index = ChunkIndex {
        chunks,
        last_rebuilt: None,
    };

    let mut group = c.benchmark_group("Chunk Migration");
    group.sample_size(10);
    group.measurement_time(std::time::Duration::from_secs(20));

    group.bench_function("migrate_from_json_1000", |b| {
        b.iter(|| {
            // We create a fresh DB in temp to measure realistic disk I/O sync overhead
            let iter_dir = tempdir().unwrap();
            let iter_db_path = iter_dir.path().join("iter.sqlite");
            let store = VectorStore::open(&iter_db_path).unwrap();

            store.migrate_from_json(black_box(&chunk_index)).unwrap();
        })
    });

    group.finish();
}

criterion_group!(benches, benchmark_migration);
criterion_main!(benches);
