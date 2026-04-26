use criterion::{black_box, criterion_group, criterion_main, Criterion};
use futures::stream::StreamExt;
use std::time::Duration;
use tokio::runtime::Runtime;

async fn mock_embedding_task(delay: Duration) -> Option<Vec<f32>> {
    tokio::time::sleep(delay).await;
    Some(vec![0.1; 768])
}

fn bench_embedding_optimization(c: &mut Criterion) {
    let num_tasks = 10;
    let mock_delay = Duration::from_millis(50);
    let rt = Runtime::new().unwrap();

    let mut group = c.benchmark_group("embedding_optimization");
    group.sample_size(10); // Low sample size because each iteration is slow

    group.bench_function("sequential_embeddings", |b| {
        b.to_async(&rt).iter(|| async {
            let mut results = Vec::with_capacity(num_tasks);
            for _ in 0..num_tasks {
                results.push(black_box(mock_embedding_task(mock_delay).await));
            }
            black_box(results);
        })
    });

    group.bench_function("parallel_ordered_embeddings_4", |b| {
        b.to_async(&rt).iter(|| async {
            let stream = futures::stream::iter(0..num_tasks)
                .map(|_| mock_embedding_task(mock_delay))
                .buffered(4);
            let results: Vec<_> = stream.collect().await;
            black_box(results);
        })
    });

    group.finish();
}

criterion_group!(benches, bench_embedding_optimization);
criterion_main!(benches);
