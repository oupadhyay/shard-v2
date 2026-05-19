//! Phase 1.1 — Lifecycle hooks dispatcher overhead.
//!
//! Acceptance target (from docs/plans/self_editing_harness_plan.md):
//!  * <5 µs per `dispatch_pre_tool` call at 5 hooks (all no-op `Continue`).

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use serde_json::json;
use shard_lib::agent::hooks::{
    HookOutcome, HookRegistry, LifecycleHooks, ToolInvocation, ToolOutcome,
};
use std::sync::Arc;

struct NoopHook;
impl LifecycleHooks for NoopHook {
    fn on_pre_tool_use(&self, _call: &ToolInvocation<'_>) -> HookOutcome {
        HookOutcome::Continue
    }
    fn on_post_tool_use(&self, _outcome: &ToolOutcome<'_>) {}
}

fn make_registry(n: usize) -> HookRegistry {
    let mut r = HookRegistry::new();
    for _ in 0..n {
        r.push(Arc::new(NoopHook));
    }
    r
}

fn bench_pre_tool_dispatch(c: &mut Criterion) {
    let mut group = c.benchmark_group("hook_dispatch_pre_tool");
    let args = json!({"q": "hello world"});

    for &n in &[0usize, 1, 5, 10] {
        let registry = make_registry(n);
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            let inv = ToolInvocation {
                name: "web_search",
                args: &args,
                call_id: None,
            };
            b.iter(|| {
                let outcome = registry.dispatch_pre_tool(black_box(&inv));
                black_box(outcome);
            });
        });
    }
    group.finish();
}

fn bench_post_tool_dispatch(c: &mut Criterion) {
    let mut group = c.benchmark_group("hook_dispatch_post_tool");
    let args = json!({"q": "hello world"});

    for &n in &[0usize, 1, 5, 10] {
        let registry = make_registry(n);
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            let outcome = ToolOutcome {
                name: "web_search",
                args: &args,
                call_id: None,
                result: "ok",
                is_error: false,
            };
            b.iter(|| {
                registry.dispatch_post_tool(black_box(&outcome));
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_pre_tool_dispatch, bench_post_tool_dispatch);
criterion_main!(benches);
