//! Phase 1.2 — Dedup window hot-path benchmarks.
//!
//! Acceptance targets (from docs/plans/self_editing_harness_plan.md):
//!  * duplicate hit: <2 µs (in-memory HashMap hit + UPDATE)
//!  * fresh insert: <30 µs (HashMap insert + INSERT OR REPLACE)

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use shard_lib::dedup::{is_duplicate, reset_hot_cache_for_testing, DedupKind};
use shard_lib::vector_store::VectorStore;
use std::time::Duration;
use tempfile::tempdir;

fn open_bench_store() -> (VectorStore, tempfile::TempDir) {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("dedup_bench.sqlite");
    let store = VectorStore::open(&db_path).unwrap();
    (store, dir)
}

fn bench_duplicate_hit(c: &mut Criterion) {
    let (store, _dir) = open_bench_store();
    reset_hot_cache_for_testing();
    // Seed once so every measured call is a hit.
    let _ = is_duplicate(
        &store,
        "bench-hot-hash",
        DedupKind::Observation,
        Duration::from_secs(300),
    );

    c.bench_function("dedup_duplicate_hit", |b| {
        b.iter(|| {
            let dup = is_duplicate(
                black_box(&store),
                black_box("bench-hot-hash"),
                black_box(DedupKind::Observation),
                black_box(Duration::from_secs(300)),
            );
            black_box(dup);
        });
    });
}

fn bench_fresh_insert(c: &mut Criterion) {
    let (store, _dir) = open_bench_store();
    reset_hot_cache_for_testing();

    let mut counter = 0u64;
    c.bench_function("dedup_fresh_insert", |b| {
        b.iter(|| {
            counter = counter.wrapping_add(1);
            let key = format!("fresh-{}", counter);
            let dup = is_duplicate(
                black_box(&store),
                black_box(&key),
                black_box(DedupKind::Observation),
                black_box(Duration::from_secs(300)),
            );
            black_box(dup);
        });
    });
}

criterion_group!(benches, bench_duplicate_hit, bench_fresh_insert);
criterion_main!(benches);
