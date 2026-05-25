//! Phase 2.3 — rollback_self_edit / file_history snapshot tests.
//!
//! Uses real `tempfile`-backed paths so the rollback's `std::fs::write`
//! works against a writable disk location.

use crate::file_history::{record_edit, rollback_event, RecordEdit, SNAPSHOT_SIZE_CAP};
use crate::vector_store::VectorStore;
use rusqlite::params;
use std::fs;
use tempfile::tempdir;

fn open() -> (VectorStore, tempfile::TempDir) {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("rollback.sqlite");
    let store = VectorStore::open(&db_path).unwrap();
    (store, dir)
}

fn make_file(dir: &tempfile::TempDir, name: &str, contents: &str) -> String {
    let path = dir.path().join(name);
    fs::write(&path, contents).unwrap();
    path.display().to_string()
}

#[test]
fn rollback_most_recent_edit_restores_content() {
    let (store, dir) = open();
    let abs = make_file(&dir, "config.toml", "after");
    let id = record_edit(
        &store,
        RecordEdit {
            logical_path: "config.toml",
            abs_path: &abs,
            before: "before",
            after: "after",
            unified_diff: "diff",
            session_id: None,
        },
    )
    .unwrap();

    let (reverted_id, len) = rollback_event(&store, "config.toml", None).unwrap();
    assert_eq!(reverted_id, id);
    assert_eq!(len, "before".len());
    assert_eq!(fs::read_to_string(&abs).unwrap(), "before");
}

#[test]
fn rollback_specific_event_id_works() {
    let (store, dir) = open();
    let abs = make_file(&dir, "config.toml", "v3");
    let id1 = record_edit(
        &store,
        RecordEdit {
            logical_path: "config.toml",
            abs_path: &abs,
            before: "v1",
            after: "v2",
            unified_diff: "diff1",
            session_id: None,
        },
    )
    .unwrap();
    let _id2 = record_edit(
        &store,
        RecordEdit {
            logical_path: "config.toml",
            abs_path: &abs,
            before: "v2",
            after: "v3",
            unified_diff: "diff2",
            session_id: None,
        },
    )
    .unwrap();

    // Roll back to id1 specifically — should restore v1.
    let (reverted_id, _len) = rollback_event(&store, "config.toml", Some(&id1)).unwrap();
    assert_eq!(reverted_id, id1);
    assert_eq!(fs::read_to_string(&abs).unwrap(), "v1");
}

#[test]
fn rollback_records_revert_event() {
    let (store, dir) = open();
    let abs = make_file(&dir, "config.toml", "modified");
    record_edit(
        &store,
        RecordEdit {
            logical_path: "config.toml",
            abs_path: &abs,
            before: "original",
            after: "modified",
            unified_diff: "diff",
            session_id: None,
        },
    )
    .unwrap();
    rollback_event(&store, "config.toml", None).unwrap();

    let count: i64 = store
        .conn
        .query_row(
            "SELECT COUNT(*) FROM file_events WHERE logical_path = 'config.toml' AND event_kind = 'revert'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn rollback_errors_when_no_restorable_edit_exists() {
    let (store, _dir) = open();
    let err = rollback_event(&store, "config.toml", None).unwrap_err();
    assert!(err.contains("no restorable edit"));
}

#[test]
fn rollback_errors_when_event_id_lacks_snapshot() {
    let (store, dir) = open();
    let abs = make_file(&dir, "config.toml", "after");
    // Insert an edit row but force before_content to NULL (simulating a
    // pre-2.3 row or a >SNAPSHOT_SIZE_CAP edit).
    let id = record_edit(
        &store,
        RecordEdit {
            logical_path: "config.toml",
            abs_path: &abs,
            before: "before",
            after: "after",
            unified_diff: "diff",
            session_id: None,
        },
    )
    .unwrap();
    store
        .conn
        .execute(
            "UPDATE file_events SET before_content = NULL WHERE id = ?",
            params![id],
        )
        .unwrap();

    let err = rollback_event(&store, "config.toml", Some(&id)).unwrap_err();
    assert!(err.contains("no rollback target"));
}

#[test]
fn record_edit_skips_snapshot_for_oversized_files() {
    let (store, dir) = open();
    let abs = make_file(&dir, "config.toml", "after");
    let big_before = "x".repeat(SNAPSHOT_SIZE_CAP + 1);
    let id = record_edit(
        &store,
        RecordEdit {
            logical_path: "config.toml",
            abs_path: &abs,
            before: &big_before,
            after: "after",
            unified_diff: "diff",
            session_id: None,
        },
    )
    .unwrap();
    let stored: Option<String> = store
        .conn
        .query_row(
            "SELECT before_content FROM file_events WHERE id = ?",
            params![id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(stored.is_none(), "oversized edits must not store a snapshot");

    // And rollback for that specific id must fail loudly.
    let err = rollback_event(&store, "config.toml", Some(&id)).unwrap_err();
    assert!(err.contains("no rollback target"));
}

#[test]
fn rollback_at_size_cap_boundary_works() {
    let (store, dir) = open();
    let abs = make_file(&dir, "config.toml", "after");
    // Exactly at the cap → snapshot stored.
    let before = "y".repeat(SNAPSHOT_SIZE_CAP);
    record_edit(
        &store,
        RecordEdit {
            logical_path: "config.toml",
            abs_path: &abs,
            before: &before,
            after: "after",
            unified_diff: "d",
            session_id: None,
        },
    )
    .unwrap();
    let (_id, len) = rollback_event(&store, "config.toml", None).unwrap();
    assert_eq!(len, SNAPSHOT_SIZE_CAP);
    assert_eq!(fs::read_to_string(&abs).unwrap().len(), SNAPSHOT_SIZE_CAP);
}
