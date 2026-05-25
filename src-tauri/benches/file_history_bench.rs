//! Phase 2.1/2.3 — file_history insertion + summary + rollback benchmarks.
//!
//! Acceptance targets (docs/plans/self_editing_harness_plan.md):
//!  * summary over 1k events: <10 ms
//!  * record_edit insert: <500 µs (SQLite WAL fsync bound)
//!  * rollback_event (most-recent): <500 µs

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use shard_lib::file_history::{
    record_edit, record_read, rollback_event, summarize, RecordEdit, RecordRead,
};
use shard_lib::vector_store::VectorStore;
use std::fs;
use tempfile::tempdir;

fn open_store() -> (VectorStore, tempfile::TempDir) {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("file_history.sqlite");
    let store = VectorStore::open(&db_path).unwrap();
    (store, dir)
}

fn populate(store: &VectorStore, n: usize) {
    for i in 0..n {
        if i % 4 == 0 {
            let _ = record_edit(
                store,
                RecordEdit {
                    logical_path: "config.toml",
                    abs_path: "/tmp/config.toml",
                    before: &format!("v{}", i),
                    after: &format!("v{}", i + 1),
                    unified_diff: &format!("@@ -1 +1 @@\n-v{}\n+v{}\n", i, i + 1),
                    session_id: Some("bench-session"),
                },
            );
        } else {
            let _ = record_read(
                store,
                RecordRead {
                    logical_path: "config.toml",
                    abs_path: "/tmp/config.toml",
                    content: &format!("contents {}", i),
                    session_id: Some("bench-session"),
                },
            );
        }
    }
}

fn bench_summarize(c: &mut Criterion) {
    let mut group = c.benchmark_group("file_history_summarize");
    for &n in &[100usize, 1_000] {
        let (store, _dir) = open_store();
        populate(&store, n);
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| {
                let s = summarize(black_box(&store), "config.toml", 25).unwrap();
                black_box(s);
            });
        });
    }
    group.finish();
}

fn bench_record_edit(c: &mut Criterion) {
    let (store, _dir) = open_store();
    c.bench_function("file_history_record_edit", |b| {
        let mut counter = 0u64;
        b.iter(|| {
            counter = counter.wrapping_add(1);
            let _ = record_edit(
                black_box(&store),
                RecordEdit {
                    logical_path: "config.toml",
                    abs_path: "/tmp/config.toml",
                    before: "old",
                    after: "new",
                    unified_diff: "@@ -1 +1 @@\n-old\n+new\n",
                    session_id: None,
                },
            );
        });
    });
}

fn bench_rollback(c: &mut Criterion) {
    let (store, dir) = open_store();
    // Create a real on-disk file because rollback_event writes through fs.
    let abs = dir.path().join("config.toml");
    fs::write(&abs, "after").unwrap();
    let abs_str = abs.display().to_string();
    // Seed a single restorable edit.
    let _ = record_edit(
        &store,
        RecordEdit {
            logical_path: "config.toml",
            abs_path: &abs_str,
            before: "before",
            after: "after",
            unified_diff: "diff",
            session_id: None,
        },
    );
    c.bench_function("file_history_rollback_latest", |b| {
        b.iter(|| {
            // Repeatedly rolling back the same row is fine — the file
            // contents converge to "before" after the first iter.
            let _ = rollback_event(black_box(&store), "config.toml", None);
        });
    });
}

criterion_group!(benches, bench_summarize, bench_record_edit, bench_rollback);
criterion_main!(benches);
