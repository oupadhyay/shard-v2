//! Phase 3.1 — Action / Frontier planner tests.

use crate::actions::{
    block, complete, count_by_status, frontier, get, insert_action, plan, sketch_children,
    update_status, ActionStatus,
};
use crate::vector_store::VectorStore;
use tempfile::tempdir;

fn open() -> (VectorStore, tempfile::TempDir) {
    let dir = tempdir().unwrap();
    let store = VectorStore::open(&dir.path().join("actions.sqlite")).unwrap();
    (store, dir)
}

#[test]
fn status_roundtrip() {
    for s in &[
        ActionStatus::Pending,
        ActionStatus::Active,
        ActionStatus::Done,
        ActionStatus::Blocked,
        ActionStatus::Cancelled,
    ] {
        assert_eq!(ActionStatus::parse(s.as_str()), Some(*s));
    }
    assert!(ActionStatus::parse("nope").is_none());
}

#[test]
fn insert_lone_action_appears_on_frontier() {
    let (store, _dir) = open();
    let id = insert_action(&store, None, "do thing", &[], 0, None, None).unwrap();
    let front = frontier(&store).unwrap().expect("frontier should not be empty");
    assert_eq!(front.id, id);
    assert_eq!(front.status, ActionStatus::Pending);
}

#[test]
fn frontier_returns_highest_priority_unblocked() {
    let (store, _dir) = open();
    let low = insert_action(&store, None, "low pri", &[], 0, None, None).unwrap();
    let high = insert_action(&store, None, "high pri", &[], 10, None, None).unwrap();
    let mid = insert_action(&store, None, "mid pri", &[], 5, None, None).unwrap();

    let front = frontier(&store).unwrap().unwrap();
    assert_eq!(front.id, high);

    complete(&store, &high, Some("ok")).unwrap();
    let front = frontier(&store).unwrap().unwrap();
    assert_eq!(front.id, mid);

    complete(&store, &mid, Some("ok")).unwrap();
    let front = frontier(&store).unwrap().unwrap();
    assert_eq!(front.id, low);
}

#[test]
fn dependencies_block_until_satisfied() {
    let (store, _dir) = open();
    let parent = insert_action(&store, None, "parent", &[], 0, None, None).unwrap();
    let child = insert_action(
        &store,
        None,
        "child needs parent done",
        &[parent.clone()],
        10, // higher priority but blocked by dep
        None,
        None,
    )
    .unwrap();

    // Parent has lower priority but no deps, so it's on the frontier first.
    let front = frontier(&store).unwrap().unwrap();
    assert_eq!(front.id, parent);

    complete(&store, &parent, Some("done")).unwrap();
    let front = frontier(&store).unwrap().unwrap();
    assert_eq!(front.id, child);
}

#[test]
fn blocked_actions_do_not_appear_on_frontier() {
    let (store, _dir) = open();
    let a = insert_action(&store, None, "a", &[], 5, None, None).unwrap();
    let b = insert_action(&store, None, "b", &[], 0, None, None).unwrap();
    block(&store, &a, "user input needed").unwrap();
    let front = frontier(&store).unwrap().unwrap();
    assert_eq!(front.id, b);
}

#[test]
fn frontier_returns_none_when_everything_done_or_blocked() {
    let (store, _dir) = open();
    let id = insert_action(&store, None, "single", &[], 0, None, None).unwrap();
    complete(&store, &id, None).unwrap();
    assert!(frontier(&store).unwrap().is_none());

    let blocked = insert_action(&store, None, "blocked", &[], 0, None, None).unwrap();
    block(&store, &blocked, "wait").unwrap();
    assert!(frontier(&store).unwrap().is_none());
}

#[test]
fn plan_creates_chained_sketch() {
    let (store, _dir) = open();
    let ids = plan(
        &store,
        "Rename persona analyst→senior_analyst",
        &["rename in personas/", "update referencing config keys", "verify with file_history"],
        Some("sess-1"),
    )
    .unwrap();
    assert_eq!(ids.len(), 4); // parent + 3 children

    let parent = ids[0].clone();
    let children = sketch_children(&store, &parent).unwrap();
    assert_eq!(children.len(), 3);
    // Steps are chained: step N depends on step N-1.
    assert!(children[0].deps.is_empty());
    assert_eq!(children[1].deps, vec![children[0].id.clone()]);
    assert_eq!(children[2].deps, vec![children[1].id.clone()]);

    // The first step has highest priority among children.
    assert!(children[0].priority > children[1].priority);
}

#[test]
fn plan_then_walk_frontier_in_order() {
    let (store, _dir) = open();
    let ids = plan(&store, "demo", &["step a", "step b", "step c"], None).unwrap();
    let child_ids = &ids[1..];

    // Parent has priority 0 and no deps → it's the FIRST frontier item.
    // step a (priority 0) and parent (priority 0) tie; the parent was
    // inserted first so created_at orders it first.
    // Complete the parent immediately so the walk feels natural for tests.
    complete(&store, &ids[0], Some("parent ack")).unwrap();

    for expected in child_ids {
        let front = frontier(&store).unwrap().unwrap();
        assert_eq!(front.id, *expected);
        complete(&store, &front.id, Some("ok")).unwrap();
    }
    assert!(frontier(&store).unwrap().is_none());
}

#[test]
fn cyclic_dependency_rejected_at_insert() {
    let (store, _dir) = open();
    let a = insert_action(&store, None, "a", &[], 0, None, None).unwrap();
    // Mutate `a` to depend on itself via SQL (simulating a corrupted insert
    // path) and then try to insert b → a; b's validation should walk the
    // cycle and refuse.
    store
        .conn
        .execute(
            "UPDATE actions SET deps = ? WHERE id = ?",
            rusqlite::params![serde_json::json!([&a]).to_string(), a],
        )
        .unwrap();
    let err = insert_action(&store, None, "b", &[a.clone()], 0, None, None).unwrap_err();
    assert!(err.contains("cyclic"));
}

#[test]
fn count_by_status_filters_correctly() {
    let (store, _dir) = open();
    let a = insert_action(&store, None, "a", &[], 0, None, None).unwrap();
    let _b = insert_action(&store, None, "b", &[], 0, None, None).unwrap();
    complete(&store, &a, None).unwrap();

    assert_eq!(count_by_status(&store, ActionStatus::Pending).unwrap(), 1);
    assert_eq!(count_by_status(&store, ActionStatus::Done).unwrap(), 1);
    assert_eq!(count_by_status(&store, ActionStatus::Blocked).unwrap(), 0);
}

#[test]
fn update_status_changes_state_and_updated_at() {
    let (store, _dir) = open();
    let id = insert_action(&store, None, "x", &[], 0, None, None).unwrap();
    let before = get(&store, &id).unwrap().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(15));
    update_status(&store, &id, ActionStatus::Active, None, None).unwrap();
    let after = get(&store, &id).unwrap().unwrap();
    assert_eq!(after.status, ActionStatus::Active);
    assert_ne!(after.updated_at, before.updated_at);
}

#[test]
fn update_status_errors_on_missing_id() {
    let (store, _dir) = open();
    let err = update_status(&store, "missing", ActionStatus::Done, None, None).unwrap_err();
    assert!(err.contains("not found"));
}

#[test]
fn multi_root_sketch_returns_one_per_root() {
    // Two independent sketches; pending_sketch_summary must return exactly one
    // entry per root and not collapse them, while frontier still returns the
    // single highest-priority action across both roots.
    let (store, _dir) = open();

    let ids_a = plan(&store, "Refactor persona files", &["scan", "rename"], None).unwrap();
    let ids_b = plan(&store, "Tidy heartbeats", &["audit", "rewrite"], None).unwrap();
    assert_ne!(ids_a[0], ids_b[0]);

    // Complete the parent rows so children own the frontier; this is the
    // natural state once the agent acks each sketch.
    complete(&store, &ids_a[0], None).unwrap();
    complete(&store, &ids_b[0], None).unwrap();

    let sketches = crate::actions::pending_sketch_summary(&store).unwrap();
    assert_eq!(sketches.len(), 2, "expected one summary entry per open root");

    let mut roots: Vec<String> = sketches.iter().map(|s| s.root_id.clone()).collect();
    roots.sort();
    let mut expected = vec![ids_a[0].clone(), ids_b[0].clone()];
    expected.sort();
    assert_eq!(roots, expected);

    for s in &sketches {
        assert_eq!(s.total, 2);
        assert_eq!(s.completed, 0);
        assert!(s.next_action_id.is_some());
    }

    // Render produces text with both roots represented.
    let text = crate::actions::pending_sketch_summary_text(&store).unwrap();
    assert!(text.contains("Refactor persona files"));
    assert!(text.contains("Tidy heartbeats"));
}

#[test]
fn compaction_preserves_open_sketch() {
    // The pre-compaction hook captures the open sketch via
    // pending_sketch_summary_text. After a simulated compaction (which clears
    // in-memory chat history but cannot touch the persistent actions table),
    // both the frontier and the summary are recoverable, so the agent can
    // resume the multi-step plan post-flush.
    let dir = tempdir().unwrap();
    let path = dir.path().join("actions.sqlite");
    let store = VectorStore::open(&path).unwrap();
    let ids = plan(
        &store,
        "Rename analyst → senior_analyst",
        &["edit persona file", "update config refs", "verify"],
        Some("sess-compact"),
    )
    .unwrap();
    complete(&store, &ids[0], None).unwrap(); // ack parent
    complete(&store, &ids[1], Some("renamed")).unwrap(); // first step done

    // What the pre_compact hook would snapshot:
    let snapshot_before = crate::actions::pending_sketch_summary_text(&store)
        .expect("open sketch should produce snapshot");
    assert!(snapshot_before.contains("Rename analyst"));
    assert!(snapshot_before.contains("update config refs"));

    // Simulate compaction by dropping the in-memory store (analogous to the
    // chat history being summarized) and re-opening from the same path.
    // The SQLite-backed actions table is durable across the flush.
    drop(store);
    let reopened = VectorStore::open(&path).unwrap();

    let snapshot_after = crate::actions::pending_sketch_summary_text(&reopened)
        .expect("sketch should still be open after reopen");
    assert!(snapshot_after.contains("Rename analyst"));
    assert!(snapshot_after.contains("update config refs"));

    // Frontier walks straight into the next pending step.
    let next = frontier(&reopened).unwrap().expect("frontier should resume");
    assert_eq!(next.id, ids[2]);
}
