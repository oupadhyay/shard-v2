//! Phase 3.2 — Crystals pipeline microbench.
//!
//! Measures the cost of the synthesis steps that wrap an LLM call so we
//! can guarantee the periodic sweep stays cheap even when an eligible
//! sketch shows up. The LLM itself is the long pole (~1 s) and is mocked
//! out here — the benches only cover what Shard does on either side.
//!
//! Acceptance (from `docs/plans/self_editing_harness_plan.md`):
//!   pipeline (ex-LLM) <15 ms

use criterion::{criterion_group, criterion_main, BatchSize, Criterion};

use shard_lib::actions::{complete, plan, sketch_children};
use shard_lib::crystals::{
    build_prompt, find_completed_sketches, pick_unique_slug, should_crystallize_sketch,
    slugify, stamp_source_sketch_id, strip_markdown_fences,
};
use shard_lib::vector_store::VectorStore;

fn seed_sketch(store: &VectorStore, title: &str, n: usize) -> Vec<String> {
    let steps: Vec<String> = (0..n).map(|i| format!("step {i}")).collect();
    let refs: Vec<&str> = steps.iter().map(|s| s.as_str()).collect();
    let ids = plan(store, title, &refs, None).unwrap();
    for cid in &ids[1..] {
        complete(store, cid, Some("ok")).unwrap();
    }
    ids
}

fn bench_slugify(c: &mut Criterion) {
    c.bench_function("crystals_slugify_short", |b| {
        b.iter(|| {
            let _ = slugify("Rename `analyst` persona to `senior_analyst` everywhere");
        })
    });
}

fn bench_pick_unique_slug(c: &mut Criterion) {
    let existing: Vec<String> = (0..1000).map(|i| format!("recipe-{i}")).collect();
    c.bench_function("crystals_pick_unique_slug_1k_existing", |b| {
        b.iter(|| {
            // Forces 11 retries (`recipe-0`..`recipe-10` all taken until 11).
            let _ = pick_unique_slug("recipe-0", &existing);
        })
    });
}

fn bench_build_prompt(c: &mut Criterion) {
    let dir = tempfile::tempdir().unwrap();
    let store = VectorStore::open(&dir.path().join("crystals_bench.sqlite")).unwrap();
    let ids = seed_sketch(&store, "Crystals bench sketch", 20);
    let children = sketch_children(&store, &ids[0]).unwrap();
    c.bench_function("crystals_build_prompt_20_children", |b| {
        b.iter(|| {
            let _ = build_prompt("Crystals bench sketch", &children);
        })
    });
}

fn bench_decision_predicate(c: &mut Criterion) {
    let dir = tempfile::tempdir().unwrap();
    let store = VectorStore::open(&dir.path().join("crystals_decision.sqlite")).unwrap();
    let ids = seed_sketch(&store, "All-done sketch", 20);
    let parent = shard_lib::actions::get(&store, &ids[0]).unwrap().unwrap();
    let kids = sketch_children(&store, &ids[0]).unwrap();
    c.bench_function("crystals_should_crystallize_20_children", |b| {
        b.iter(|| {
            let _ = should_crystallize_sketch(&parent, &kids);
        })
    });
}

fn bench_stamp_and_strip(c: &mut Criterion) {
    let raw = "```markdown\n---\ndescription: x\n---\n\n# body\n".repeat(8);
    c.bench_function("crystals_strip_then_stamp", |b| {
        b.iter(|| {
            let inner = strip_markdown_fences(&raw);
            let _ = stamp_source_sketch_id(&inner, "abc-123");
        })
    });
}

fn bench_full_pipeline_ex_llm(c: &mut Criterion) {
    // The 15 ms acceptance covers every step that runs around the
    // network call: load_sketch + should_crystallize + slugify +
    // pick_unique_slug + build_prompt + strip + stamp.
    c.bench_function("crystals_full_pipeline_ex_llm_20_children", |b| {
        b.iter_batched(
            || {
                let dir = tempfile::tempdir().unwrap();
                let store =
                    VectorStore::open(&dir.path().join("pipeline.sqlite")).unwrap();
                let ids = seed_sketch(&store, "Demo full sketch", 20);
                (dir, store, ids[0].clone())
            },
            |(_dir, store, sketch_id)| {
                let parent = shard_lib::actions::get(&store, &sketch_id)
                    .unwrap()
                    .unwrap();
                let kids = sketch_children(&store, &sketch_id).unwrap();
                let _ = should_crystallize_sketch(&parent, &kids);
                let base = slugify(&parent.title).unwrap();
                let _slug = pick_unique_slug(&base, &[]).unwrap();
                let prompt = build_prompt(&parent.title, &kids);
                assert!(!prompt.is_empty());
                // Simulate the LLM-return-shape massaging.
                let raw = "```\n---\ndescription: x\nrequired_tools:\n  - action_plan\n---\n```";
                let inner = strip_markdown_fences(raw);
                let _ = stamp_source_sketch_id(&inner, &parent.id);
            },
            BatchSize::SmallInput,
        )
    });
}

fn bench_find_completed_sketches(c: &mut Criterion) {
    let dir = tempfile::tempdir().unwrap();
    let store = VectorStore::open(&dir.path().join("sweep.sqlite")).unwrap();
    for i in 0..50 {
        seed_sketch(&store, &format!("Sketch {i}"), 5);
    }
    c.bench_function("crystals_find_completed_sketches_50", |b| {
        b.iter(|| {
            let _ = find_completed_sketches(&store);
        })
    });
}

criterion_group!(
    benches,
    bench_slugify,
    bench_pick_unique_slug,
    bench_build_prompt,
    bench_decision_predicate,
    bench_stamp_and_strip,
    bench_full_pipeline_ex_llm,
    bench_find_completed_sketches
);
criterion_main!(benches);
