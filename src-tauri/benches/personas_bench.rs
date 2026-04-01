use criterion::{criterion_group, criterion_main, Criterion};
use shard_lib::personas::{get_persona_content, list_available_personas};

fn bench_list_skills(c: &mut Criterion) {
    c.bench_function("list_available_personas", |b| {
        b.iter(|| list_available_personas())
    });
}

fn bench_get_skill_content(c: &mut Criterion) {
    c.bench_function("get_skill_content_nonexistent", |b| {
        b.iter(|| get_persona_content("nonexistent_skill_for_benchmarking"))
    });
}

criterion_group!(benches, bench_list_skills, bench_get_skill_content);
criterion_main!(benches);
