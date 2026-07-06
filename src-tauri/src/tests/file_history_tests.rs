//! Phase 2.1 — file_history event log + summary tests.

use crate::file_history::{
    attribute_error_to_recent_edits, get_events, record_edit, record_read, summarize,
    FileEventKind, RecordEdit, RecordRead,
};
use crate::vector_store::VectorStore;
use rusqlite::params;
use tempfile::tempdir;

fn open_store() -> (VectorStore, tempfile::TempDir) {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("file_history.sqlite");
    let store = VectorStore::open(&db_path).unwrap();
    (store, dir)
}

#[test]
fn read_event_logged_with_hash() {
    let (store, _dir) = open_store();
    let id = record_read(
        &store,
        RecordRead {
            logical_path: "config.toml",
            abs_path: "/tmp/config.toml",
            content: "hello world",
            session_id: Some("s1"),
        },
    )
    .unwrap();
    let events = get_events(&store, "config.toml", 10).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].id, id);
    assert_eq!(events[0].event_kind, FileEventKind::Read);
    assert!(events[0].after_hash.is_some());
    assert!(events[0].unified_diff.is_none());
}

#[test]
fn edit_event_logs_diff_and_hashes() {
    let (store, _dir) = open_store();
    let _id = record_edit(
        &store,
        RecordEdit {
            logical_path: "config.toml",
            abs_path: "/tmp/config.toml",
            before: "selected_model = \"a\"",
            after: "selected_model = \"b\"",
            unified_diff: "@@ -1 +1 @@\n-selected_model = \"a\"\n+selected_model = \"b\"\n",
            session_id: Some("s1"),
        },
    )
    .unwrap();
    let events = get_events(&store, "config.toml", 10).unwrap();
    assert_eq!(events.len(), 1);
    let ev = &events[0];
    assert_eq!(ev.event_kind, FileEventKind::Edit);
    assert!(ev.before_hash.is_some());
    assert!(ev.after_hash.is_some());
    assert_ne!(ev.before_hash, ev.after_hash);
    assert!(ev
        .unified_diff
        .as_deref()
        .unwrap()
        .contains("+selected_model"));
}

#[test]
fn error_within_window_attaches_to_last_edit() {
    let (store, _dir) = open_store();
    let _id = record_edit(
        &store,
        RecordEdit {
            logical_path: "config.toml",
            abs_path: "/tmp/config.toml",
            before: "x = 1",
            after: "x = 2",
            unified_diff: "diff",
            session_id: None,
        },
    )
    .unwrap();
    let n = attribute_error_to_recent_edits(&store, "config.toml", "compile error").unwrap();
    assert_eq!(n, 1);
    let events = get_events(&store, "config.toml", 10).unwrap();
    assert_eq!(
        events[0].followed_by_error.as_deref(),
        Some("compile error")
    );
}

#[test]
fn error_outside_window_does_not_attach() {
    let (store, _dir) = open_store();
    let id = record_edit(
        &store,
        RecordEdit {
            logical_path: "config.toml",
            abs_path: "/tmp/config.toml",
            before: "",
            after: "x",
            unified_diff: "d",
            session_id: None,
        },
    )
    .unwrap();
    // Push created_at 10 minutes into the past (outside the 60s window).
    let old_ts = (chrono::Utc::now() - chrono::Duration::minutes(10)).to_rfc3339();
    store
        .conn
        .execute(
            "UPDATE file_events SET created_at = ? WHERE id = ?",
            params![old_ts, id],
        )
        .unwrap();
    let n = attribute_error_to_recent_edits(&store, "config.toml", "later error").unwrap();
    assert_eq!(n, 0);
    let events = get_events(&store, "config.toml", 10).unwrap();
    assert!(events[0].followed_by_error.is_none());
}

#[test]
fn error_does_not_overwrite_existing_attribution() {
    let (store, _dir) = open_store();
    let _ = record_edit(
        &store,
        RecordEdit {
            logical_path: "config.toml",
            abs_path: "/tmp/config.toml",
            before: "",
            after: "x",
            unified_diff: "d",
            session_id: None,
        },
    )
    .unwrap();
    attribute_error_to_recent_edits(&store, "config.toml", "first").unwrap();
    // A second error in the window should NOT overwrite — the query filters
    // by `followed_by_error IS NULL`.
    let n = attribute_error_to_recent_edits(&store, "config.toml", "second").unwrap();
    assert_eq!(n, 0);
    let events = get_events(&store, "config.toml", 10).unwrap();
    assert_eq!(events[0].followed_by_error.as_deref(), Some("first"));
}

#[test]
fn summarize_empty_history_says_so() {
    let (store, _dir) = open_store();
    let s = summarize(&store, "config.toml", 10).unwrap();
    assert!(s.contains("No prior reads"));
}

#[test]
fn summarize_groups_by_outcome_and_warns_on_error() {
    let (store, _dir) = open_store();
    record_edit(
        &store,
        RecordEdit {
            logical_path: "config.toml",
            abs_path: "/tmp/config.toml",
            before: "a",
            after: "b",
            unified_diff: "diff1",
            session_id: None,
        },
    )
    .unwrap();
    attribute_error_to_recent_edits(&store, "config.toml", "boom").unwrap();
    record_read(
        &store,
        RecordRead {
            logical_path: "config.toml",
            abs_path: "/tmp/config.toml",
            content: "anything",
            session_id: None,
        },
    )
    .unwrap();

    let s = summarize(&store, "config.toml", 10).unwrap();
    assert!(s.contains("1 edit, 1 read, 1 edit followed by a tool error"));
    assert!(s.contains("⚠️ Caution"));
    assert!(s.contains("**Followed by error:** boom"));
}

#[test]
fn summarize_respects_limit() {
    let (store, _dir) = open_store();
    for i in 0..20 {
        record_read(
            &store,
            RecordRead {
                logical_path: "config.toml",
                abs_path: "/tmp/config.toml",
                content: &format!("contents {}", i),
                session_id: None,
            },
        )
        .unwrap();
    }
    let s = summarize(&store, "config.toml", 5).unwrap();
    // 5 events rendered, all reads; no edits → no caution.
    assert!(s.contains("0 edits"));
    assert!(s.contains("most recent 5 shown"));
    assert!(!s.contains("⚠️ Caution"));
}

#[test]
fn summarize_truncates_long_diff() {
    let (store, _dir) = open_store();
    let big_diff = "+".repeat(2000);
    record_edit(
        &store,
        RecordEdit {
            logical_path: "config.toml",
            abs_path: "/tmp/config.toml",
            before: "x",
            after: "y",
            unified_diff: &big_diff,
            session_id: None,
        },
    )
    .unwrap();
    let s = summarize(&store, "config.toml", 5).unwrap();
    assert!(s.contains("... (diff truncated)"));
}
