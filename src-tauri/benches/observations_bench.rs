use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use shard_lib::observations::{
    get_observations_by_level, get_recent_observations, get_top_derived_observations,
    insert_observation, make_observation, search_observations_by_embedding,
    search_observations_by_keyword, ObservationLevel,
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
    vec![seed; 768]
}

fn populate_store(store: &VectorStore, count: usize) -> Vec<String> {
    let mut ids = Vec::with_capacity(count);
    for i in 0..count {
        let obs = make_observation(
            &format!("Observation fact number {} about the user preferences and behavior patterns", i),
            ObservationLevel::Explicit,
            vec![],
            None,
        );
        let emb = make_test_embedding(0.01 * (i as f32 % 100.0));
        ids.push(obs.id.clone());
        insert_observation(store, &obs, Some(&emb)).unwrap();
    }

    // Add derivations so get_top_derived has data
    for i in 0..(count / 10) {
        let derived = make_observation(
            &format!("Derived insight {} from base observations", i),
            ObservationLevel::Deductive,
            vec![ids[i].clone()],
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
                    let res =
                        get_top_derived_observations(black_box(store), "user", 50).unwrap();
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

criterion_group!(benches, bench_retrieval_functions, bench_search_functions);
criterion_main!(benches);
