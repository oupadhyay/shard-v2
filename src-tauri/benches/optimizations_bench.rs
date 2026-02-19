use criterion::{black_box, criterion_group, criterion_main, Criterion};
use futures::stream::StreamExt;
use shard_lib::memories::{Memory, MemoryCategory, MemoryStore};
use std::sync::{Arc, RwLock};
use std::time::Duration;

// 1. Image Buffer Cloning vs Arc Sharing
fn bench_image_sharing(c: &mut Criterion) {
    let size = 3840 * 2160 * 4; // 4K RGBA ~33MB
    let data = vec![255u8; size];

    let mut group = c.benchmark_group("image_buffer_management");

    group.bench_function("cloning_33mb", |b| {
        b.iter(|| {
            let cloned = black_box(data.clone());
            black_box(cloned);
        })
    });

    let shared = Arc::new(data);
    group.bench_function("arc_sharing_33mb", |b| {
        b.iter(|| {
            let shared_clone = black_box(Arc::clone(&shared));
            black_box(shared_clone);
        })
    });

    group.finish();
}

// 2. Memory Store Caching
fn bench_memory_caching(c: &mut Criterion) {
    let mut store = MemoryStore::new();
    for i in 0..100 {
        store.add(Memory::new(
            MemoryCategory::Fact,
            format!(
                "Important fact #{} about the project architecture and the system design",
                i
            ),
            3,
        ));
    }

    let cached_store = Arc::new(RwLock::new(Some(store.clone())));

    let mut group = c.benchmark_group("memory_access");

    group.bench_function("rwlock_cache_read", |b| {
        b.iter(|| {
            let guard = cached_store.read().unwrap();
            let _ = black_box(guard.as_ref().unwrap().clone());
        })
    });

    // We don't bench disk I/O here to keep benches fast and deterministic,
    // but the contrast between RwLock (nanoseconds) and Disk (milliseconds) is the point.

    group.finish();
}

// 3. Concurrent Embedding Batch Processing
// Mocking the async workload to simulate API calls
async fn mock_embedding_task(id: usize, delay: Duration) -> usize {
    tokio::time::sleep(delay).await;
    id
}

fn bench_embedding_concurrency(c: &mut Criterion) {
    let num_tasks = 20;
    let mock_delay = Duration::from_millis(10);

    let mut group = c.benchmark_group("embedding_concurrency");

    group.bench_function("sequential_processing", |b| {
        b.to_async(tokio::runtime::Runtime::new().unwrap())
            .iter(|| async {
                for i in 0..num_tasks {
                    black_box(mock_embedding_task(i, mock_delay).await);
                }
            })
    });

    group.bench_function("concurrent_processing_4", |b| {
        b.to_async(tokio::runtime::Runtime::new().unwrap())
            .iter(|| async {
                let stream = futures::stream::iter(0..num_tasks)
                    .map(|i| mock_embedding_task(i, mock_delay))
                    .buffer_unordered(4);
                let _results: Vec<_> = stream.collect().await;
                black_box(_results);
            })
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_image_sharing,
    bench_memory_caching,
    bench_embedding_concurrency
);
criterion_main!(benches);
