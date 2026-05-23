//! Phase 3.3 — MCP server façade microbench.
//!
//! Measures the cost of the curated tool dispatch surface that runs
//! around the per-call SQLite work. No real MCP transport — we benchmark
//! the handlers directly because that's where the latency budget lives;
//! the rmcp framing cost is constant per call.
//!
//! Acceptance (from `docs/plans/self_editing_harness_plan.md`):
//!   `list_tools` <2 ms
//!   `memory_search` cold <50 ms, warm <10 ms
//!   `read_file` <5 ms

use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use serde_json::json;

use shard_lib::mcp::{
    handle_action_next, handle_action_plan, handle_file_history, handle_memory_search,
    handle_read_file, shard_data_dir, shard_db_path, ShardMcpServer,
};

/// Force `$HOME` to a tempdir so the MCP path helpers resolve inside the
/// bench sandbox instead of polluting real user data. Returned guard
/// restores `$HOME` on drop.
struct HomeJail {
    _td: tempfile::TempDir,
    prev: Option<std::ffi::OsString>,
}
impl HomeJail {
    fn new() -> Self {
        let td = tempfile::Builder::new()
            .prefix("shard-mcp-bench-")
            .tempdir()
            .unwrap();
        let prev = std::env::var_os("HOME");
        std::env::set_var("HOME", td.path());
        Self { _td: td, prev }
    }
}
impl Drop for HomeJail {
    fn drop(&mut self) {
        match &self.prev {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }
}

fn bench_list_tools(c: &mut Criterion) {
    c.bench_function("mcp_list_curated_tools", |b| {
        b.iter(|| {
            let _ = ShardMcpServer::list_curated_tools();
        })
    });
}

fn bench_memory_search_cold_and_warm(c: &mut Criterion) {
    // Cold: open a fresh DB and run search. VectorStore::open does WAL
    // init + sqlite-vec extension load, so the first call per process
    // pays the full setup cost.
    c.bench_function("mcp_memory_search_cold", |b| {
        b.iter_batched(
            || HomeJail::new(),
            |_jail| {
                let _ = handle_memory_search(&json!({ "query": "octopus", "limit": 5 }));
            },
            BatchSize::PerIteration,
        )
    });

    // Warm: keep the same jail across iterations; only the open() per
    // call measures.
    {
        let _jail = HomeJail::new();
        // Pre-open + drop so the file is on disk and warm in the page
        // cache.
        {
            let db = shard_db_path().unwrap();
            std::fs::create_dir_all(db.parent().unwrap()).unwrap();
            let _store = shard_lib::vector_store::VectorStore::open(&db).unwrap();
        }

        c.bench_function("mcp_memory_search_warm", |b| {
            b.iter(|| {
                let _ = handle_memory_search(&json!({ "query": "octopus", "limit": 5 }));
            })
        });
    }
}

fn bench_read_file(c: &mut Criterion) {
    let _jail = HomeJail::new();
    let cfg = shard_data_dir().unwrap().join("config.toml");
    std::fs::write(&cfg, "selected_model = \"x\"\n".repeat(100)).unwrap();

    c.bench_function("mcp_read_file_config_toml_2kb", |b| {
        b.iter(|| {
            let _ = handle_read_file(&json!({ "path": "config.toml" }));
        })
    });
}

fn bench_file_history_summary(c: &mut Criterion) {
    // Seed file_events with N rows so summarize() has work to do.
    let _jail = HomeJail::new();
    let db = shard_db_path().unwrap();
    std::fs::create_dir_all(db.parent().unwrap()).unwrap();
    let store = shard_lib::vector_store::VectorStore::open(&db).unwrap();
    for _ in 0..50 {
        shard_lib::file_history::record_edit(
            &store,
            shard_lib::file_history::RecordEdit {
                logical_path: "config.toml",
                abs_path: "/tmp/config.toml",
                before: "x",
                after: "y",
                unified_diff: "--- a\n+++ b\n",
                session_id: Some("bench"),
            },
        )
        .unwrap();
    }
    drop(store);

    c.bench_function("mcp_file_history_summary_50_events", |b| {
        b.iter(|| {
            let _ = handle_file_history(&json!({ "path": "config.toml", "limit": 50 }));
        })
    });
}

fn bench_action_dispatch(c: &mut Criterion) {
    let _jail = HomeJail::new();
    c.bench_function("mcp_action_plan_then_next", |b| {
        b.iter(|| {
            let _ = handle_action_plan(&json!({
                "title": "bench-sketch",
                "steps": ["a", "b", "c"],
            }));
            let _ = handle_action_next(&json!({}));
        })
    });
}

criterion_group!(
    benches,
    bench_list_tools,
    bench_memory_search_cold_and_warm,
    bench_read_file,
    bench_file_history_summary,
    bench_action_dispatch,
);
criterion_main!(benches);
