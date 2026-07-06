//! Phase 3.3 — MCP tool handler implementations.
//!
//! Each `handle_*` function takes raw JSON arguments (whatever the MCP
//! client sent) and returns either a success string (rendered as MCP
//! `Content::text`) or a structured error. They are plain async functions
//! that go straight at the SQLite store / filesystem via the path helpers
//! in [`super`], deliberately bypassing the Tauri `AppHandle` so the MCP
//! loop runs in a headless context.
//!
//! The handlers stay narrowly focused — anything that needs LLM access
//! (e.g. `memory_search`'s embedding generation, `crystallize_sketch`,
//! `web_search`) is intentionally absent from the curated MCP subset.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::actions;
use crate::file_history;
use crate::self_files;
use crate::vector_store::VectorStore;

use super::{resolve_allowed_path_no_tauri, shard_data_dir, shard_db_path};

/// Curated tool surface exposed over MCP. Heartbeat-only and draft-gated
/// tools (`create_heartbeat`, `crystallize_sketch`, `rollback_self_edit`,
/// `wake_me_up_in`, …) are intentionally absent.
pub const CURATED_TOOL_NAMES: &[&str] = &[
    "memory_search",
    "save_memory",
    "file_history",
    "read_file",
    "edit_file",
    "action_next",
    "action_plan",
];

/// Open the shared SQLite store. Wrapped in `Arc` so the calling server
/// can cache the handle across tool calls if it wants — every handler in
/// this module opens fresh, since rusqlite connections are cheap to open
/// against a WAL-mode DB.
pub fn open_store() -> Result<Arc<VectorStore>, String> {
    let path = shard_db_path()?;
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent).map_err(|e| format!("create memories dir: {}", e))?;
        }
    }
    let store = VectorStore::open(&path)
        .map_err(|e| format!("Failed to open vector store at {:?}: {}", path, e))?;
    Ok(Arc::new(store))
}

// ─── memory_search ────────────────────────────────────────────────────────

/// FTS5-only search over `chunks`. We deliberately skip the dense-vector
/// path here — MCP clients shouldn't have to provide a Gemini key just to
/// poke at memory. Hybrid search remains available via the Tauri agent UI.
pub fn handle_memory_search(args: &Value) -> Result<String, String> {
    let query = args
        .get("query")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "memory_search requires `query` (string)".to_string())?;
    if query.trim().is_empty() {
        return Err("memory_search `query` must be non-empty".to_string());
    }
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|n| n.clamp(1, 50) as i64)
        .unwrap_or(10);

    let store = open_store()?;
    let mut stmt = store
        .conn
        .prepare(
            "SELECT c.source_type, c.source_name, c.heading, c.text, \
                    bm25(chunks_fts) AS score \
             FROM chunks_fts JOIN chunks c ON c.id = chunks_fts.chunk_id \
             WHERE chunks_fts MATCH ? \
             ORDER BY score LIMIT ?",
        )
        .map_err(|e| format!("prepare failed: {}", e))?;

    let rows: Vec<Value> = stmt
        .query_map(rusqlite::params![query, limit], |r| {
            Ok(json!({
                "source_type": r.get::<_, String>(0).unwrap_or_default(),
                "source_name": r.get::<_, String>(1).unwrap_or_default(),
                "heading": r.get::<_, Option<String>>(2).ok().flatten(),
                "text": r.get::<_, String>(3).unwrap_or_default(),
                "score": r.get::<_, f64>(4).unwrap_or(0.0),
            }))
        })
        .map_err(|e| format!("query failed: {}", e))?
        .filter_map(|r| r.ok())
        .collect();

    if rows.is_empty() {
        return Ok(format!(
            "No memory chunks matched `{}`. Try a broader query.",
            query
        ));
    }
    Ok(json!({ "matches": rows }).to_string())
}

// ─── save_memory ──────────────────────────────────────────────────────────

/// Append a fact to `MEMORIES.json`. Simpler than the agent-side tool —
/// we don't attempt LLM-driven dedup; the user-facing summary job
/// reconciles overlapping entries on the next 6 h sweep.
pub fn handle_save_memory(args: &Value) -> Result<String, String> {
    let category = args
        .get("category")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "save_memory requires `category`".to_string())?;
    let content = args
        .get("content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "save_memory requires `content`".to_string())?;
    let importance = args
        .get("importance")
        .and_then(|v| v.as_u64())
        .map(|n| n.clamp(1, 5) as u8)
        .unwrap_or(3);

    let path = shard_data_dir()?.join("MEMORIES.json");
    let mut existing: Value = if path.exists() {
        let raw =
            std::fs::read_to_string(&path).map_err(|e| format!("read MEMORIES.json: {}", e))?;
        serde_json::from_str(&raw).unwrap_or_else(|_| json!({ "entries": [] }))
    } else {
        json!({ "entries": [] })
    };

    let entry = json!({
        "id": uuid::Uuid::new_v4().to_string(),
        "category": category,
        "content": content,
        "importance": importance,
        "created_at": chrono::Utc::now().to_rfc3339(),
        "source": "mcp",
    });

    if let Some(arr) = existing.get_mut("entries").and_then(|v| v.as_array_mut()) {
        arr.push(entry.clone());
    } else {
        existing = json!({ "entries": [entry.clone()] });
    }

    let serialized = serde_json::to_string_pretty(&existing)
        .map_err(|e| format!("serialize MEMORIES.json: {}", e))?;
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent).map_err(|e| format!("create data dir: {}", e))?;
        }
    }
    std::fs::write(&path, serialized).map_err(|e| format!("write MEMORIES.json: {}", e))?;

    Ok(format!(
        "Saved memory `{}` (category={}, importance={}).",
        entry["id"].as_str().unwrap_or(""),
        category,
        importance
    ))
}

// ─── file_history ─────────────────────────────────────────────────────────

pub fn handle_file_history(args: &Value) -> Result<String, String> {
    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "file_history requires `path`".to_string())?;
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|n| n.clamp(1, 50) as usize)
        .unwrap_or(10);

    // Same allow-list as read_file/edit_file — refuses to surface history
    // for arbitrary paths.
    self_files::validate_logical_path(path)?;

    let store = open_store()?;
    file_history::summarize(&store, path, limit)
}

// ─── read_file / edit_file ────────────────────────────────────────────────

pub fn handle_read_file(args: &Value) -> Result<String, String> {
    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "read_file requires `path`".to_string())?;
    let abs = resolve_allowed_path_no_tauri(path)?;
    if !abs.exists() {
        return Ok(String::new());
    }
    std::fs::read_to_string(&abs).map_err(|e| format!("Failed to read {}: {}", abs.display(), e))
}

pub fn handle_edit_file(args: &Value) -> Result<String, String> {
    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "edit_file requires `path`".to_string())?;
    let old_str = args
        .get("old_str")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "edit_file requires `old_str`".to_string())?;
    let new_str = args
        .get("new_str")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "edit_file requires `new_str`".to_string())?;
    let replace_all = args
        .get("replace_all")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let abs = resolve_allowed_path_no_tauri(path)?;
    let outcome = self_files::edit_at_abs_path(&abs, path, old_str, new_str, replace_all)?;

    // Best-effort: log to file_events so a future read of `file_history`
    // surfaces this edit, even though no Tauri event bus is listening.
    if let Ok(store) = open_store() {
        let abs_str = outcome.abs_path.clone();
        let _ = file_history::record_edit(
            &store,
            file_history::RecordEdit {
                logical_path: path,
                abs_path: &abs_str,
                before: &outcome.before,
                after: &outcome.after,
                unified_diff: &outcome.unified_diff,
                session_id: Some("mcp"),
            },
        );
    }

    Ok(format!(
        "{}\n\n```diff\n{}\n```",
        format_args!(
            "Edited {} ({} replacement{}).",
            outcome.abs_path,
            outcome.replacements,
            if outcome.replacements == 1 { "" } else { "s" }
        ),
        outcome.unified_diff
    ))
}

// ─── action_next / action_plan ────────────────────────────────────────────

pub fn handle_action_next(_args: &Value) -> Result<String, String> {
    let store = open_store()?;
    match actions::frontier(&store)? {
        Some(a) => Ok(json!({
            "id": a.id,
            "title": a.title,
            "priority": a.priority,
            "parent_id": a.parent_id,
            "deps": a.deps,
            "status": a.status.as_str(),
            "session_id": a.session_id,
        })
        .to_string()),
        None => Ok("null".to_string()),
    }
}

pub fn handle_action_plan(args: &Value) -> Result<String, String> {
    let title = args
        .get("title")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "action_plan requires `title`".to_string())?;
    let steps: Vec<&str> = args
        .get("steps")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();
    if steps.is_empty() {
        return Err("action_plan requires a non-empty `steps` array".to_string());
    }

    let store = open_store()?;
    let ids = actions::plan(&store, title, &steps, Some("mcp"))?;
    Ok(json!({
        "sketch_id": ids[0],
        "step_ids": &ids[1..],
        "count": steps.len(),
    })
    .to_string())
}
