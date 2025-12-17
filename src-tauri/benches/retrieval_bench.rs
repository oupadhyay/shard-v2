use criterion::{black_box, criterion_group, criterion_main, BatchSize, Criterion};
use shard_lib::retrieval::{tokenize, BM25Index};
use std::time::Duration;

fn sample_docs(n: usize) -> Vec<(String, String)> {
    // Generate a simple synthetic corpus; replace with real docs if available
    (0..n)
        .map(|i| {
            (
                format!("doc_{i}"),
                format!(
                    "Rust programming examples: ownership, lifetimes, traits. \
                     BM25 ranking test content #{i}. fn main() {{ println!(\"hello {i}\"); }} \
                     Additional tokens: unicode π, emoji 🚀, hyphen-words, CamelCase Tokens."
                ),
            )
        })
        .collect()
}

fn bench_tokenize(c: &mut Criterion) {
    let text = "This is a sample document with some code like fn main() { println!(\"hello\"); } \
                Identifiers: snake_case, CamelCase, kebab-case; numbers 12345; unicode café 🚀.";
    c.bench_function("tokenize/small", |b| b.iter(|| tokenize(black_box(text))));

    // Larger input to see scaling - use custom config for longer measurement
    let large = text.repeat(1024);
    let mut group = c.benchmark_group("tokenize");
    group.measurement_time(std::time::Duration::from_secs(10));
    group.bench_function("large_~100KB", |b| {
        b.iter(|| tokenize(black_box(&large)))
    });
    group.finish();
}

fn bench_bm25_search(c: &mut Criterion) {
    // Build index once per benchmark group
    let docs = sample_docs(1_000);
    let mut index = BM25Index::new();
    for (id, body) in &docs {
        index.add_document(id, body);
    }

    // Warm up
    for _ in 0..10 {
        let _ = index.search("Rust programming lifetimes traits", 10);
    }

    // Fixed common query
    c.bench_function("bm25_search/1k_docs_common", |b| {
        b.iter(|| index.search(black_box("Rust programming lifetimes traits"), 10))
    });

    // Short query
    c.bench_function("bm25_search/1k_docs_short", |b| {
        b.iter(|| index.search(black_box("Rust"), 10))
    });

    // OOV / rare terms to exercise IDF handling
    c.bench_function("bm25_search/1k_docs_oov", |b| {
        b.iter(|| index.search(black_box("nonexistenttoken123"), 10))
    });

    // Batched benchmark for per-query setup isolation
    c.bench_function("bm25_search/batched_readonly", |b| {
        b.iter_batched(
            || "traits ownership unicode",
            |q| index.search(black_box(q), 10),
            BatchSize::SmallInput,
        )
    });
}

fn bench_bm25_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("bm25_scaling");

    for size in [1_000, 10_000] {
        let docs = sample_docs(size);
        let mut index = BM25Index::new();
        for (id, body) in &docs {
            index.add_document(id, body);
        }

        // Warm up
        for _ in 0..5 {
            let _ = index.search("Rust programming", 10);
        }

        group.bench_function(format!("{size}_docs"), |b| {
            b.iter(|| index.search(black_box("Rust programming lifetimes"), 10))
        });
    }

    group.finish();
}

fn configure_criterion() -> Criterion {
    Criterion::default()
        .noise_threshold(0.05)     // Treat <5% change as noise
        .significance_level(0.01)  // Require p<0.01 to declare change (stricter)
        .measurement_time(Duration::from_secs(5))
}

criterion_group! {
    name = benches;
    config = configure_criterion();
    targets = bench_tokenize, bench_bm25_search, bench_bm25_scaling
}
criterion_main!(benches);
