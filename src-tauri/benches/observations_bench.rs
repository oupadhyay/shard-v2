use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use shard_lib::observations::{
    decay_score, decay_sweep, get_observations_by_level, get_recent_observations,
    get_top_derived_observations, insert_observation, make_observation, recompute_decay,
    search_observations_by_embedding, search_observations_by_keyword, ObservationLevel,
    DEFAULT_EVICT_THRESHOLD,
};
use shard_lib::vector_store::VectorStore;
use tempfile::tempdir;

fn open_bench_store() -> (VectorStore, tempfile::TempDir) {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("bench.sqlite");
    let store = VectorStore::open(&db_path).unwrap();
    (store, dir)
}

fn make_test_embedding(seed: f32) -> Vec<f32> {
    // Generate a simple deterministic pseudo-random-like embedding based on the seed.
    // This avoids all embeddings being colinear while keeping benchmarks reproducible.
    let mut embedding = Vec::with_capacity(768);
    let mut x = seed;
    for _ in 0..768 {
        // Chaotic update to introduce variation; constants chosen arbitrarily but fixed.
        x = (x * 1.324_718_f32 + 0.123_456_79_f32).sin();
        embedding.push(x);
    }
    embedding
}

fn populate_store(store: &VectorStore, count: usize) -> Vec<String> {
    let mut ids = Vec::with_capacity(count);
    for i in 0..count {
        let obs = make_observation(
            &format!(
                "Observation fact number {} about the user preferences and behavior patterns",
                i
            ),
            ObservationLevel::Explicit,
            vec![],
            None,
        );
        let emb = make_test_embedding(0.01 * (i as f32 % 100.0));
        ids.push(obs.id.clone());
        insert_observation(store, &obs, Some(&emb)).unwrap();
    }

    // Add derivations so get_top_derived has data
    for (i, source_id) in ids.iter().take(count / 10).enumerate() {
        let derived = make_observation(
            &format!("Derived insight {} from base observations", i),
            ObservationLevel::Deductive,
            vec![source_id.clone()],
            None,
        );
        insert_observation(store, &derived, None).unwrap();
    }

    ids
}

fn bench_retrieval_functions(c: &mut Criterion) {
    let mut group = c.benchmark_group("observation_retrieval");

    for count in [100, 500, 1000] {
        let (store, _dir) = open_bench_store();
        populate_store(&store, count);

        group.bench_with_input(
            BenchmarkId::new("get_recent_observations", format!("{count}_obs_limit50")),
            &store,
            |b, store| {
                b.iter(|| {
                    let res = get_recent_observations(black_box(store), "user", 50).unwrap();
                    black_box(res);
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("get_observations_by_level", format!("{count}_obs_limit50")),
            &store,
            |b, store| {
                b.iter(|| {
                    let res = get_observations_by_level(
                        black_box(store),
                        "user",
                        ObservationLevel::Explicit,
                        50,
                    )
                    .unwrap();
                    black_box(res);
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("get_top_derived", format!("{count}_obs_limit50")),
            &store,
            |b, store| {
                b.iter(|| {
                    let res = get_top_derived_observations(black_box(store), "user", 50).unwrap();
                    black_box(res);
                });
            },
        );
    }

    group.finish();
}

fn bench_search_functions(c: &mut Criterion) {
    let mut group = c.benchmark_group("observation_search");
    group.sample_size(10);

    let (store, _dir) = open_bench_store();
    populate_store(&store, 500);

    let query_emb = make_test_embedding(0.05);

    group.bench_function("embedding_search_500obs_join", |b| {
        b.iter(|| {
            let res = search_observations_by_embedding(
                black_box(&store),
                "user",
                black_box(&query_emb),
                20,
                0.5,
            )
            .unwrap();
            black_box(res);
        });
    });

    group.bench_function("keyword_search_500obs_join", |b| {
        b.iter(|| {
            let res = search_observations_by_keyword(
                black_box(&store),
                "user",
                "observation fact preferences",
                20,
            )
            .unwrap();
            black_box(res);
        });
    });

    group.finish();
}

// ============================================================================
// Phase 1.3 — Decay & eviction benches
// ============================================================================

fn bench_decay_score_pure(c: &mut Criterion) {
    // Pure-math hot path; should be sub-nanosecond after optimization.
    c.bench_function("decay_score_pure", |b| {
        b.iter(|| {
            black_box(decay_score(black_box(7.5), black_box(3)));
        });
    });
}

fn bench_recompute_decay(c: &mut Criterion) {
    let mut group = c.benchmark_group("recompute_decay");
    for &n in &[100usize, 1_000, 10_000] {
        let (store, _dir) = open_bench_store();
        populate_store(&store, n);
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| {
                let updated = recompute_decay(black_box(&store)).unwrap();
                black_box(updated);
            });
        });
    }
    group.finish();
}

fn bench_decay_sweep(c: &mut Criterion) {
    // Measure sweep against a populated store where every row is eligible.
    // We can't reach into `store.conn` from a bench (private), so we use a
    // very large threshold that catches every freshly-populated row
    // (score = 1.0 by default → set threshold > 1.0 so all are below).
    let mut group = c.benchmark_group("decay_sweep");
    for &n in &[1_000usize, 10_000] {
        let (store, _dir) = open_bench_store();
        populate_store(&store, n);
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| {
                // After the first sweep deleted_at is set for all rows; the
                // sweep predicate `deleted_at IS NULL` then filters them out
                // and subsequent iterations measure the no-op fast path,
                // which is the realistic steady state for the background job.
                let evicted = decay_sweep(black_box(&store), 2.0).unwrap();
                black_box(evicted);
            });
        });
    }
    group.finish();
}

fn bench_decay_sweep_threshold(c: &mut Criterion) {
    // Same as above but with the production threshold; verifies the fast
    // path when no rows are eligible (every row score = 1.0).
    let mut group = c.benchmark_group("decay_sweep_noop");
    for &n in &[1_000usize, 10_000] {
        let (store, _dir) = open_bench_store();
        populate_store(&store, n);
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| {
                let evicted = decay_sweep(black_box(&store), DEFAULT_EVICT_THRESHOLD).unwrap();
                black_box(evicted);
            });
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_retrieval_functions,
    bench_search_functions,
    bench_decay_score_pure,
    bench_recompute_decay,
    bench_decay_sweep,
    bench_decay_sweep_threshold,
);
criterion_main!(benches);
