//! Phase 3.1 — Action / Frontier planner benchmarks.
//!
//! Acceptance target (docs/plans/self_editing_harness_plan.md §3.1):
//!   frontier query over 1k pending actions with a 5-deep dependency graph: <3 ms.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use shard_lib::actions::{frontier, insert_action, pending_sketch_summary};
use shard_lib::vector_store::VectorStore;
use tempfile::tempdir;

fn open_store() -> (VectorStore, tempfile::TempDir) {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("actions.sqlite");
    let store = VectorStore::open(&db_path).unwrap();
    (store, dir)
}

/// Build a graph of `n` actions arranged in `chains` independent dependency
/// chains, each `depth` levels deep. Earlier nodes in a chain have higher
/// priority so the frontier walk surfaces them in order. Returns once all
/// rows are inserted; nothing is marked done so every node remains pending.
fn populate(store: &VectorStore, n: usize, depth: usize) {
    let chains = (n + depth - 1) / depth;
    for chain in 0..chains {
        let mut prev: Option<String> = None;
        for level in 0..depth {
            if chain * depth + level >= n {
                break;
            }
            let deps = prev.as_ref().map(|p| vec![p.clone()]).unwrap_or_default();
            let id = insert_action(
                store,
                None,
                &format!("c{}-l{}", chain, level),
                &deps,
                -(level as i32),
                None,
                None,
            )
            .expect("insert_action");
            prev = Some(id);
        }
    }
}

fn bench_frontier(c: &mut Criterion) {
    let mut group = c.benchmark_group("actions_frontier");
    for &n in &[100usize, 1_000] {
        let (store, _dir) = open_store();
        populate(&store, n, 5);
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| {
                let next = frontier(black_box(&store)).unwrap();
                black_box(next);
            });
        });
    }
    group.finish();
}

fn bench_pending_sketch_summary(c: &mut Criterion) {
    // The pre-compact hook calls this on every compaction. Worst case: many
    // root sketches active simultaneously. 50 roots × 5 children = 250 rows.
    let (store, _dir) = open_store();
    for i in 0..50 {
        let parent = insert_action(
            &store,
            None,
            &format!("sketch-{}", i),
            &[],
            0,
            None,
            None,
        )
        .unwrap();
        let mut prev: Option<String> = None;
        for level in 0..5 {
            let deps = prev.as_ref().map(|p| vec![p.clone()]).unwrap_or_default();
            let id = insert_action(
                &store,
                Some(&parent),
                &format!("step-{}-{}", i, level),
                &deps,
                -(level as i32),
                None,
                None,
            )
            .unwrap();
            prev = Some(id);
        }
    }
    c.bench_function("actions_pending_sketch_summary_50roots", |b| {
        b.iter(|| {
            let summary = pending_sketch_summary(black_box(&store)).unwrap();
            black_box(summary);
        });
    });
}

criterion_group!(benches, bench_frontier, bench_pending_sketch_summary);
criterion_main!(benches);
