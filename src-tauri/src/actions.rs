//! Phase 3.1 — Action / Frontier planner.
//!
//! A small persistent task graph the agent uses for multi-step self-edits
//! (e.g. "rename `analyst` persona to `senior_analyst` across config + all
//! references"). Lets the agent survive compaction — a partially-completed
//! sketch is recoverable via `frontier()` after a context flush.
//!
//! Semantics:
//!  * Statuses: `pending` → `active` → `done` (terminal) | `blocked` | `cancelled`.
//!  * `frontier()` returns the highest-priority `pending` action whose `deps`
//!    are all `done`. Soft-blocked actions (status=blocked) need explicit
//!    `unblock`/`update_status` before they re-enter the frontier.
//!  * A "sketch" is a parent action with N children — `plan(title, steps)`
//!    creates that shape and threads chain dependencies so steps
//!    execute in order by default. Callers wanting a parallel sketch can pass
//!    empty deps via [`insert_action`].

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};

use crate::vector_store::VectorStore;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActionStatus {
    Pending,
    Active,
    Done,
    Blocked,
    Cancelled,
}

impl ActionStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Active => "active",
            Self::Done => "done",
            Self::Blocked => "blocked",
            Self::Cancelled => "cancelled",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "active" => Some(Self::Active),
            "done" => Some(Self::Done),
            "blocked" => Some(Self::Blocked),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Done | Self::Cancelled)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Action {
    pub id: String,
    pub parent_id: Option<String>,
    pub title: String,
    pub status: ActionStatus,
    pub priority: i32,
    pub deps: Vec<String>,
    pub payload: Option<String>,
    pub session_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub block_reason: Option<String>,
    pub outcome: Option<String>,
}

/// Single-step action insert. Most callers should use [`plan`] instead so
/// dependencies are wired up automatically.
pub fn insert_action(
    store: &VectorStore,
    parent_id: Option<&str>,
    title: &str,
    deps: &[String],
    priority: i32,
    payload: Option<&str>,
    session_id: Option<&str>,
) -> Result<String, String> {
    validate_no_cycle(store, parent_id, deps)?;
    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    let deps_json = serde_json::to_string(deps).unwrap_or_else(|_| "[]".to_string());
    store
        .conn
        .execute(
            "INSERT INTO actions \
             (id, parent_id, title, status, priority, deps, payload, session_id, \
              created_at, updated_at, block_reason, outcome) \
             VALUES (?1, ?2, ?3, 'pending', ?4, ?5, ?6, ?7, ?8, ?8, NULL, NULL)",
            params![id, parent_id, title, priority, deps_json, payload, session_id, now],
        )
        .map_err(|e| format!("insert_action failed: {}", e))?;
    Ok(id)
}

/// Plan a sketch: a parent action plus N children chained in order. Returns
/// the parent id followed by the child ids. This is the typical entry point
/// for `action_plan` from the agent's tool surface.
pub fn plan(
    store: &VectorStore,
    title: &str,
    steps: &[&str],
    session_id: Option<&str>,
) -> Result<Vec<String>, String> {
    let parent = insert_action(store, None, title, &[], 0, None, session_id)?;
    let mut ids = vec![parent.clone()];
    let mut prev: Option<String> = None;
    for (i, step_title) in steps.iter().enumerate() {
        let deps = prev.as_ref().map(|p| vec![p.clone()]).unwrap_or_default();
        let cid = insert_action(
            store,
            Some(&parent),
            step_title,
            &deps,
            -(i as i32), // earlier steps have higher priority
            None,
            session_id,
        )?;
        ids.push(cid.clone());
        prev = Some(cid);
    }
    Ok(ids)
}

/// The "next action" — highest-priority pending action whose deps are all
/// `done`. Returns `None` if the queue is empty or every pending action is
/// blocked.
pub fn frontier(store: &VectorStore) -> Result<Option<Action>, String> {
    let candidates = load_pending(store)?;
    let done = load_done_ids(store)?;

    // Sort pending by (priority DESC, created_at ASC) so the SQL index gives
    // us the right order on read, and we just filter in Rust.
    for action in candidates {
        if action.deps.iter().all(|d| done.contains(d)) {
            return Ok(Some(action));
        }
    }
    Ok(None)
}

/// All actions belonging to a sketch (`parent_id = root_id`), in their
/// natural execution order (priority DESC, created_at ASC).
pub fn sketch_children(store: &VectorStore, parent_id: &str) -> Result<Vec<Action>, String> {
    let mut stmt = store
        .conn
        .prepare(
            "SELECT id, parent_id, title, status, priority, deps, payload, \
                    session_id, created_at, updated_at, block_reason, outcome \
             FROM actions WHERE parent_id = ? \
             ORDER BY priority DESC, created_at ASC",
        )
        .map_err(|e| e.to_string())?;
    let rows: Vec<Action> = stmt
        .query_map(params![parent_id], |r| Ok(row_to_action(r)))
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

pub fn update_status(
    store: &VectorStore,
    id: &str,
    status: ActionStatus,
    outcome: Option<&str>,
    block_reason: Option<&str>,
) -> Result<(), String> {
    let now = chrono::Utc::now().to_rfc3339();
    let n = store
        .conn
        .execute(
            "UPDATE actions SET status = ?, outcome = ?, block_reason = ?, updated_at = ? \
             WHERE id = ?",
            params![status.as_str(), outcome, block_reason, now, id],
        )
        .map_err(|e| format!("update_status failed: {}", e))?;
    if n == 0 {
        return Err(format!("action {} not found", id));
    }
    Ok(())
}

pub fn complete(store: &VectorStore, id: &str, outcome: Option<&str>) -> Result<(), String> {
    update_status(store, id, ActionStatus::Done, outcome, None)
}

pub fn block(store: &VectorStore, id: &str, reason: &str) -> Result<(), String> {
    update_status(store, id, ActionStatus::Blocked, None, Some(reason))
}

pub fn get(store: &VectorStore, id: &str) -> Result<Option<Action>, String> {
    store
        .conn
        .query_row(
            "SELECT id, parent_id, title, status, priority, deps, payload, \
                    session_id, created_at, updated_at, block_reason, outcome \
             FROM actions WHERE id = ?",
            params![id],
            |r| Ok(row_to_action(r)),
        )
        .optional()
        .map_err(|e| e.to_string())
}

/// Compact summary of one open sketch (parent action with pending or active
/// children). Returned by [`pending_sketch_summary`] for the pre-compaction
/// hook so the agent's open multi-step plans survive the context flush.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SketchSummary {
    pub root_id: String,
    pub title: String,
    pub completed: usize,
    pub total: usize,
    /// `None` when every child is done — the sketch is logically complete but
    /// the parent hasn't been marked `done` yet.
    pub next_action_id: Option<String>,
    pub next_action_title: Option<String>,
}

/// Returns one [`SketchSummary`] per open sketch. A sketch is "open" when at
/// least one of its child actions is still non-terminal (pending, active, or
/// blocked) — regardless of the parent's own status, since the agent
/// typically acks the parent (`action_complete`) as soon as it picks up the
/// sketch.
///
/// Cancelled sketches are skipped wholesale (the parent's `cancelled` status
/// is treated as a stop-the-line marker).
pub fn pending_sketch_summary(store: &VectorStore) -> Result<Vec<SketchSummary>, String> {
    let mut roots_stmt = store
        .conn
        .prepare(
            "SELECT id, title, status FROM actions \
             WHERE parent_id IS NULL AND status != 'cancelled' \
             ORDER BY priority DESC, created_at ASC",
        )
        .map_err(|e| e.to_string())?;
    let roots: Vec<(String, String)> = roots_stmt
        .query_map([], |r| {
            let id: String = r.get(0)?;
            let title: String = r.get(1)?;
            Ok((id, title))
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();

    let done_ids = load_done_ids(store)?;
    let mut out = Vec::with_capacity(roots.len());
    for (root_id, title) in roots {
        let children = sketch_children(store, &root_id)?;
        if children.is_empty() {
            continue;
        }
        let completed = children
            .iter()
            .filter(|c| matches!(c.status, ActionStatus::Done))
            .count();
        // No open children → the sketch is logically complete; skip.
        if completed == children.len() {
            continue;
        }
        let next = children.iter().find(|c| {
            matches!(c.status, ActionStatus::Pending) && c.deps.iter().all(|d| done_ids.contains(d))
        });
        out.push(SketchSummary {
            root_id,
            title,
            completed,
            total: children.len(),
            next_action_id: next.map(|a| a.id.clone()),
            next_action_title: next.map(|a| a.title.clone()),
        });
    }
    Ok(out)
}

/// Human-readable rendering of [`pending_sketch_summary`] suitable for both
/// the daily-log pre-compaction flush and the per-turn system context blob.
/// Returns `None` when no sketches are open so callers can no-op cleanly.
pub fn pending_sketch_summary_text(store: &VectorStore) -> Option<String> {
    let sketches = pending_sketch_summary(store).ok()?;
    if sketches.is_empty() {
        return None;
    }
    let mut out = String::from("Open action sketches:\n");
    for s in sketches {
        out.push_str(&format!(
            "- [{}/{}] {} (id={})",
            s.completed, s.total, s.title, s.root_id
        ));
        if let (Some(nid), Some(ntitle)) = (s.next_action_id, s.next_action_title) {
            out.push_str(&format!("\n  → next: {} (id={})", ntitle, nid));
        }
        out.push('\n');
    }
    Some(out)
}

/// Count actions in a given status; used by `pre_compact` hooks and the
/// `action_status` tool.
pub fn count_by_status(store: &VectorStore, status: ActionStatus) -> Result<usize, String> {
    store
        .conn
        .query_row(
            "SELECT COUNT(*) FROM actions WHERE status = ?",
            params![status.as_str()],
            |r| r.get::<_, i64>(0).map(|n| n as usize),
        )
        .map_err(|e| e.to_string())
}

// ─── internals ───────────────────────────────────────────────────────────

fn load_pending(store: &VectorStore) -> Result<Vec<Action>, String> {
    let mut stmt = store
        .conn
        .prepare(
            "SELECT id, parent_id, title, status, priority, deps, payload, \
                    session_id, created_at, updated_at, block_reason, outcome \
             FROM actions WHERE status = 'pending' \
             ORDER BY priority DESC, created_at ASC",
        )
        .map_err(|e| e.to_string())?;
    let rows: Vec<Action> = stmt
        .query_map([], |r| Ok(row_to_action(r)))
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

fn load_done_ids(store: &VectorStore) -> Result<HashSet<String>, String> {
    let mut stmt = store
        .conn
        .prepare("SELECT id FROM actions WHERE status = 'done'")
        .map_err(|e| e.to_string())?;
    let rows: HashSet<String> = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

fn row_to_action(r: &rusqlite::Row) -> Action {
    let deps_str: String = r.get(5).unwrap_or_else(|_| "[]".to_string());
    let deps: Vec<String> = serde_json::from_str(&deps_str).unwrap_or_default();
    Action {
        id: r.get(0).unwrap_or_default(),
        parent_id: r.get(1).ok(),
        title: r.get(2).unwrap_or_default(),
        status: ActionStatus::parse(&r.get::<_, String>(3).unwrap_or_default())
            .unwrap_or(ActionStatus::Pending),
        priority: r.get::<_, i64>(4).unwrap_or(0) as i32,
        deps,
        payload: r.get(6).ok(),
        session_id: r.get(7).ok(),
        created_at: r.get(8).unwrap_or_default(),
        updated_at: r.get(9).unwrap_or_default(),
        block_reason: r.get(10).ok(),
        outcome: r.get(11).ok(),
    }
}

/// Refuse to insert an action whose deps would form a cycle. We only need
/// to validate the dep graph we're appending to — existing rows are assumed
/// acyclic (enforced inductively by this same check on every prior insert).
fn validate_no_cycle(
    store: &VectorStore,
    _parent_id: Option<&str>,
    deps: &[String],
) -> Result<(), String> {
    if deps.is_empty() {
        return Ok(());
    }
    // Pull the entire dep graph (cheap — actions are small N) and BFS from
    // every dep looking for any path that comes back to a hypothetical new
    // node. Because the new node has no ancestors yet, the only way to form
    // a cycle is if two of the deps are mutually reachable, which would
    // already violate the existing graph's invariant — but we check anyway.
    let mut stmt = store
        .conn
        .prepare("SELECT id, deps FROM actions")
        .map_err(|e| e.to_string())?;
    let edges: HashMap<String, Vec<String>> = stmt
        .query_map([], |r| {
            let id: String = r.get(0)?;
            let raw: String = r.get(1).unwrap_or_else(|_| "[]".to_string());
            let parsed: Vec<String> = serde_json::from_str(&raw).unwrap_or_default();
            Ok((id, parsed))
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();

    let dep_set: HashSet<&String> = deps.iter().collect();
    for start in deps {
        let mut frontier: VecDeque<&String> = VecDeque::from([start]);
        let mut seen: HashSet<&String> = HashSet::from([start]);
        while let Some(node) = frontier.pop_front() {
            if let Some(parents) = edges.get(node) {
                for p in parents {
                    // Cycle check must come BEFORE the `seen` insert: `start`
                    // is pre-seeded into `seen`, so a self-loop (start → start)
                    // or a longer cycle that revisits `start` would otherwise
                    // be silently skipped.
                    if p == start {
                        return Err(format!("cyclic dependency detected on action {}", start));
                    }
                    if dep_set.contains(p) && p != start {
                        // Two deps share an ancestor path — fine. Mutual
                        // reachability among deps means the existing graph
                        // is already broken, but that's not a new cycle.
                    }
                    if !seen.insert(p) {
                        continue;
                    }
                    frontier.push_back(p);
                }
            }
        }
    }
    Ok(())
}
