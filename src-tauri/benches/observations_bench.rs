use criterion::{black_box, criterion_group, criterion_main, Criterion};
use shard_lib::observations::{insert_observation, make_observation, get_top_derived_observations, ObservationLevel};
use shard_lib::vector_store::VectorStore;
use tempfile::tempdir;

fn bench_get_top_derived(c: &mut Criterion) {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.sqlite");
    let store = VectorStore::open(&db_path).unwrap();

    // Populate with 1000 observations
    let mut ids = Vec::new();
    for i in 0..1000 {
        let obs = make_observation(&format!("Fact {}", i), ObservationLevel::Explicit, vec![], None);
        ids.push(obs.id.clone());
        insert_observation(&store, &obs, None).unwrap();
    }

    // Add some derivations to make them "top derived"
    for i in 0..100 {
        let source_ids = vec![ids[i].clone()];
        let derived = make_observation("Derived fact", ObservationLevel::Deductive, source_ids, None);
        insert_observation(&store, &derived, None).unwrap();
    }

    c.bench_function("get_top_derived_1000_limit_100", |b| {
        b.iter(|| {
            let res = get_top_derived_observations(black_box(&store), "user", black_box(100)).unwrap();
            black_box(res);
        })
    });
}

criterion_group!(benches, bench_get_top_derived);
criterion_main!(benches);
