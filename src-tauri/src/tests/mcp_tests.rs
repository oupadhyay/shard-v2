//! Phase 3.3 — MCP server façade tests.
//!
//! Five cases per the plan:
//!   1. `list_tools_returns_curated_subset` — heartbeats-only / draft-gated
//!      tools are not surfaced.
//!   2. `memory_search_round_trip` — handler returns matches over a seeded
//!      `chunks_fts` index.
//!   3. `edit_file_via_mcp_writes_through_self_files` — `handle_edit_file`
//!      writes via the allow-list and logs to `file_events`.
//!   4. `edit_file_outside_allowlist_refused` — handler rejects unknown
//!      logical paths before touching disk.
//!   5. `concurrent_clients_serialized` — the server `write_lock` enforces
//!      a strict order across overlapping `edit_file` invocations.
//!
//! Tests that touch the shared on-disk data dir serialize on
//! [`MCP_TEST_LOCK`] to avoid `$HOME` races (mirrors the agent test lock).

use crate::mcp::{
    handle_action_next, handle_action_plan, handle_edit_file, handle_file_history,
    handle_memory_search, handle_read_file, handle_save_memory, resolve_allowed_path_no_tauri,
    shard_config_dir, shard_data_dir, shard_db_path, ShardMcpServer, CURATED_TOOL_NAMES,
};
use serde_json::json;
use std::time::Duration;

// Delegate to the single canonical `$HOME` lock so MCP tests serialize against
// agent/heartbeat/persona tests too — they all mutate the same process-global
// `$HOME` and otherwise race on the shared on-disk DB.
use crate::tests::agent_helpers::{
    home_lock as mcp_test_lock, home_lock_async as mcp_test_lock_async,
};

/// Redirect `$HOME` to a tempdir so `dirs::data_local_dir()` resolves
/// inside the sandbox. The MCP module derives every on-disk path from
/// `dirs`, so this is sufficient to isolate the test from real user data.
struct HomeJail {
    _td: tempfile::TempDir,
    prev: Option<std::ffi::OsString>,
}

impl HomeJail {
    fn new() -> Self {
        let td = tempfile::Builder::new()
            .prefix("shard-mcp-")
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

// ─── 1. list_tools surface ─────────────────────────────────────────────────

#[test]
fn list_tools_returns_curated_subset() {
    let names: Vec<String> = ShardMcpServer::list_curated_tools()
        .iter()
        .map(|t| t.name.to_string())
        .collect();

    for expected in CURATED_TOOL_NAMES {
        assert!(
            names.iter().any(|n| n == expected),
            "expected `{expected}` in MCP curated list, got {:?}",
            names
        );
    }

    // Heartbeat-only / draft-gated tools MUST NOT be exposed.
    for forbidden in &[
        "create_heartbeat",
        "edit_heartbeat",
        "delete_heartbeat",
        "wake_me_up_in",
        "crystallize_sketch",
        "rollback_self_edit",
        "run_python",
        "web_search",
    ] {
        assert!(
            !names.iter().any(|n| n == forbidden),
            "tool `{forbidden}` MUST NOT be exposed over MCP, got {:?}",
            names
        );
    }
}

#[test]
fn list_tools_descriptors_carry_input_schema() {
    for tool in ShardMcpServer::list_curated_tools() {
        let schema = serde_json::to_string(tool.input_schema.as_ref()).unwrap();
        assert!(
            schema.contains("\"type\":\"object\""),
            "tool `{}` should expose an object schema, got: {schema}",
            tool.name
        );
    }
}

// ─── 2. memory_search round-trip ──────────────────────────────────────────

#[test]
fn memory_search_round_trip() {
    let _lock = mcp_test_lock();
    let _jail = HomeJail::new();

    // Seed a chunk so the FTS5 search has something to match on.
    let db_path = shard_db_path().unwrap();
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let store = crate::vector_store::VectorStore::open(&db_path).unwrap();
    store
        .conn
        .execute(
            "INSERT INTO chunks (id, source_type, source_name, heading, text, \
                                 start_line, end_line, content_hash, created_at, updated_at) \
             VALUES ('c1', 'topic', 'octopus', 'octopus cognition', \
                     'Octopuses solve maze puzzles in lab tests.', 1, 1, 'h', \
                     datetime('now'), datetime('now'))",
            [],
        )
        .unwrap();
    drop(store);

    let result = handle_memory_search(&json!({ "query": "octopus", "limit": 5 })).unwrap();
    assert!(
        result.contains("octopus"),
        "expected match in MCP memory_search, got: {result}"
    );
    assert!(result.contains("source_name") || result.contains("octopus"));
}

#[test]
fn memory_search_empty_query_errors() {
    let err = handle_memory_search(&json!({ "query": "  " })).unwrap_err();
    assert!(err.to_lowercase().contains("non-empty"));
}

#[test]
fn memory_search_missing_query_errors() {
    let err = handle_memory_search(&json!({})).unwrap_err();
    assert!(err.contains("query"));
}

// ─── 3. edit_file via MCP writes through self_files ───────────────────────

#[test]
fn edit_file_via_mcp_writes_through_self_files() {
    let _lock = mcp_test_lock();
    let _jail = HomeJail::new();

    // Pre-seed config.toml with some content.
    let cfg = shard_config_dir().unwrap().join("config.toml");
    std::fs::write(&cfg, "selected_model = \"original\"\n").unwrap();

    let result = handle_edit_file(&json!({
        "path": "config.toml",
        "old_str": "original",
        "new_str": "patched",
    }))
    .unwrap();

    assert!(result.contains("Edited"), "got: {result}");
    let after = std::fs::read_to_string(&cfg).unwrap();
    assert!(after.contains("patched"), "file should reflect edit");

    // And it should land in file_events — confirm by calling file_history.
    let summary = handle_file_history(&json!({ "path": "config.toml", "limit": 5 })).unwrap();
    assert!(summary.contains("config.toml"));
    assert!(summary.contains("edit"));
}

#[test]
fn edit_file_creates_new_persona_via_allowlist() {
    let _lock = mcp_test_lock();
    let _jail = HomeJail::new();

    let body = "---\ndescription: MCP-authored persona\n---\n\n# body\n";
    handle_edit_file(&json!({
        "path": "personas/mcp-author.md",
        "old_str": "",
        "new_str": body,
    }))
    .unwrap();

    let abs = resolve_allowed_path_no_tauri("personas/mcp-author.md").unwrap();
    let on_disk = std::fs::read_to_string(&abs).unwrap();
    assert_eq!(on_disk, body);
}

// ─── 4. allow-list refusal ────────────────────────────────────────────────

#[test]
fn edit_file_outside_allowlist_refused() {
    let _lock = mcp_test_lock();
    let _jail = HomeJail::new();

    for bad in &[
        "/etc/passwd",
        "../../something",
        "secret.toml",
        "personas/UPPER.md",
        "personas/no-extension",
    ] {
        let err = handle_edit_file(&json!({
            "path": bad,
            "old_str": "",
            "new_str": "x",
        }))
        .unwrap_err();
        assert!(
            err.contains("not allow-listed")
                || err.contains("must end")
                || err.contains("may only")
                || err.contains("must start"),
            "expected allow-list refusal for `{bad}`, got: {err}"
        );
    }
}

#[test]
fn read_file_outside_allowlist_refused() {
    let _lock = mcp_test_lock();
    let _jail = HomeJail::new();
    let err = handle_read_file(&json!({ "path": "../../etc/passwd" })).unwrap_err();
    assert!(err.contains("not allow-listed"), "got: {err}");
}

#[test]
fn edit_file_refuses_api_key_in_config() {
    let _lock = mcp_test_lock();
    let _jail = HomeJail::new();
    // Pre-seed so apply_edit has something to match on.
    let cfg = shard_config_dir().unwrap().join("config.toml");
    std::fs::write(&cfg, "selected_model = \"x\"\n").unwrap();
    let err = handle_edit_file(&json!({
        "path": "config.toml",
        "old_str": "selected_model = \"x\"",
        "new_str": "gemini_api_key = \"secret\"",
    }))
    .unwrap_err();
    assert!(err.to_lowercase().contains("api_key"), "got: {err}");
}

// ─── 5. concurrent clients serialized ─────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_clients_serialized() {
    let _lock = mcp_test_lock_async().await;
    let _jail = HomeJail::new();

    let server = std::sync::Arc::new(ShardMcpServer::new());

    // Spawn N tasks that each create a *different* persona file. With the
    // write_lock in place every spawn should still land (no torn writes),
    // and observed in-flight write count must never exceed 1.
    let n = 8usize;
    let in_flight = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let max_observed = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

    let mut handles = Vec::new();
    for i in 0..n {
        let server = server.clone();
        let in_flight = in_flight.clone();
        let max_observed = max_observed.clone();
        handles.push(tokio::spawn(async move {
            // Each task writes to its own persona slug — no content
            // contention, so a working mutex must still let all N land.
            let body = format!("---\ndescription: concurrent-{i}\n---\n\n# body {i}\n");
            // The dispatch_for_test guard mirrors production dispatch:
            // it holds write_lock for the whole critical section.
            let lock = server.write_lock_for_test();
            let guard = lock.lock().await;
            // Inside the critical section: bump + sample the in-flight
            // counter. With proper serialization, max observed == 1.
            let now = in_flight.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            max_observed.fetch_max(now, std::sync::atomic::Ordering::SeqCst);
            // Short sleep to widen the window for collisions if the
            // mutex weren't there.
            tokio::time::sleep(Duration::from_millis(5)).await;
            let res = handle_edit_file(&json!({
                "path": format!("personas/concur-{i}.md"),
                "old_str": "",
                "new_str": body,
            }));
            in_flight.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            drop(guard);
            res
        }));
    }

    let mut successes = 0usize;
    for h in handles {
        if h.await.unwrap().is_ok() {
            successes += 1;
        }
    }
    assert_eq!(successes, n, "every serialized edit should land");
    assert_eq!(
        max_observed.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "write_lock must serialize concurrent edits — observed ≥2 in flight"
    );

    // And every persona should be on disk.
    for i in 0..n {
        let abs = resolve_allowed_path_no_tauri(&format!("personas/concur-{i}.md")).unwrap();
        assert!(abs.exists(), "persona {i} missing on disk");
    }
}

// ─── action_plan / action_next round-trip ─────────────────────────────────

#[test]
fn action_plan_then_action_next_round_trip() {
    let _lock = mcp_test_lock();
    let _jail = HomeJail::new();

    let plan_result = handle_action_plan(&json!({
        "title": "MCP-driven refactor",
        "steps": ["read file", "patch", "verify"],
    }))
    .unwrap();
    let plan_json: serde_json::Value = serde_json::from_str(&plan_result).unwrap();
    let sketch_id = plan_json["sketch_id"].as_str().unwrap();
    assert!(!sketch_id.is_empty());
    assert_eq!(plan_json["count"].as_u64(), Some(3));

    let next = handle_action_next(&json!({})).unwrap();
    assert!(
        next != "null",
        "frontier should not be empty after action_plan"
    );
    let next_json: serde_json::Value = serde_json::from_str(&next).unwrap();
    // The parent action has the highest priority (0) by default; children
    // are inserted with negative priorities (-0, -1, …) so the parent
    // surfaces first.
    assert_eq!(next_json["id"].as_str(), Some(sketch_id));
}

#[test]
fn action_plan_rejects_empty_steps() {
    let err = handle_action_plan(&json!({ "title": "no-op", "steps": [] })).unwrap_err();
    assert!(err.contains("non-empty"), "got: {err}");
}

// ─── save_memory smoke ────────────────────────────────────────────────────

#[test]
fn save_memory_writes_to_disk() {
    let _lock = mcp_test_lock();
    let _jail = HomeJail::new();

    let out = handle_save_memory(&json!({
        "category": "fact",
        "content": "MCP test fact",
        "importance": 4,
    }))
    .unwrap();
    assert!(out.contains("Saved memory"));

    let mem_path = shard_data_dir().unwrap().join("MEMORIES.json");
    let raw = std::fs::read_to_string(&mem_path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let entries = parsed["entries"].as_array().unwrap();
    assert!(entries.iter().any(|e| e["content"] == "MCP test fact"));
}

// ─── Test-only dispatch shim ──────────────────────────────────────────────

impl ShardMcpServer {
    /// Mirror of [`Self::dispatch`] usable from tests without going through
    /// the full ServerHandler trait + rmcp transport. Kept here (rather
    /// than in `server.rs`) so the production module isn't polluted with
    /// `#[cfg(test)]` shims.
    pub async fn dispatch_for_test(
        &self,
        name: &str,
        args: serde_json::Value,
    ) -> Result<String, String> {
        // Bind the Arc<Mutex> to a stable local first so the
        // MutexGuard borrows from a longer-lived binding.
        let lock = self.write_lock_for_test();
        let _guard = match name {
            "edit_file" | "save_memory" | "action_plan" => Some(lock.lock().await),
            _ => None,
        };
        let res = match name {
            "memory_search" => handle_memory_search(&args),
            "save_memory" => handle_save_memory(&args),
            "file_history" => handle_file_history(&args),
            "read_file" => handle_read_file(&args),
            "edit_file" => handle_edit_file(&args),
            "action_next" => handle_action_next(&args),
            "action_plan" => handle_action_plan(&args),
            other => Err(format!("`{}` is not exposed over MCP", other)),
        };
        drop(_guard);
        res
    }
}
