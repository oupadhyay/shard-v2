//! Phase 1.2 — SHA-256 dedup window tests.
//!
//! Each test opens a fresh in-memory-backed VectorStore + resets the
//! in-process hot cache so they can run in any order.

use crate::dedup::{
    is_duplicate, peek_hit_count, peek_hit_count_memory, reset_hot_cache_for_testing, DedupKind,
};
use crate::vector_store::VectorStore;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Duration;
use tempfile::tempdir;

/// The dedup hot cache is process-global (matching production semantics:
/// one VectorStore per app). To keep tests deterministic when run in
/// parallel, every test in this module acquires this mutex for its
/// duration. Tests do not contend with each other in production.
fn dedup_test_guard() -> MutexGuard<'static, ()> {
    static M: OnceLock<Mutex<()>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

fn open_store() -> (VectorStore, tempfile::TempDir, MutexGuard<'static, ()>) {
    let guard = dedup_test_guard();
    reset_hot_cache_for_testing();
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("dedup.sqlite");
    let store = VectorStore::open(&db_path).unwrap();
    (store, dir, guard)
}

#[test]
fn first_call_is_not_duplicate() {
    let (store, _dir, _guard) = open_store();
    assert!(!is_duplicate(
        &store,
        "hash-a",
        DedupKind::Observation,
        Duration::from_secs(300)
    ));
}

#[test]
fn second_call_within_window_is_duplicate() {
    let (store, _dir, _guard) = open_store();
    let _ = is_duplicate(
        &store,
        "hash-a",
        DedupKind::Observation,
        Duration::from_secs(300),
    );
    assert!(is_duplicate(
        &store,
        "hash-a",
        DedupKind::Observation,
        Duration::from_secs(300)
    ));
}

#[test]
fn duplicate_after_window_is_kept() {
    let (store, _dir, _guard) = open_store();
    // Insert with a 1ms window so the second call falls outside.
    let _ = is_duplicate(
        &store,
        "hash-fast-expiry",
        DedupKind::Observation,
        Duration::from_millis(1),
    );
    std::thread::sleep(Duration::from_millis(15));
    assert!(!is_duplicate(
        &store,
        "hash-fast-expiry",
        DedupKind::Observation,
        Duration::from_millis(1)
    ));
}

#[test]
fn different_kinds_do_not_collide() {
    let (store, _dir, _guard) = open_store();
    let h = "shared-hash";
    let dup_obs = is_duplicate(&store, h, DedupKind::Observation, Duration::from_secs(300));
    let dup_tool = is_duplicate(&store, h, DedupKind::ToolResult, Duration::from_secs(300));
    // First insert per kind is never a duplicate.
    assert!(!dup_obs);
    assert!(!dup_tool);
    // But a repeat of either kind is.
    assert!(is_duplicate(
        &store,
        h,
        DedupKind::Observation,
        Duration::from_secs(300)
    ));
    assert!(is_duplicate(
        &store,
        h,
        DedupKind::ToolResult,
        Duration::from_secs(300)
    ));
}

#[test]
fn hit_count_increments_on_dup() {
    let (store, _dir, _guard) = open_store();
    let h = "incrementing";
    for _ in 0..5 {
        let _ = is_duplicate(&store, h, DedupKind::Observation, Duration::from_secs(300));
    }
    // The authoritative counter lives in the in-memory cache. The durable
    // table only reflects the initial insert (hit_count=1) and is bumped on
    // loop-warning boundaries — both ergonomic choices that keep the dup
    // hot path SQL-free. See dedup.rs for the rationale.
    assert_eq!(peek_hit_count_memory(h, DedupKind::Observation), 5);
    assert_eq!(peek_hit_count(&store, h, DedupKind::Observation), 1);
}

#[test]
fn observation_insert_dedup_suppresses_second_call() {
    use crate::observations::{
        count_observations, insert_observation_dedup, make_observation, ObservationLevel,
    };

    let (store, _dir, _guard) = open_store();
    let obs = make_observation(
        "User prefers dark mode",
        ObservationLevel::Explicit,
        vec![],
        None,
    );

    let first = insert_observation_dedup(&store, &obs, None).unwrap();
    assert!(first, "first insert should succeed");
    assert_eq!(count_observations(&store, "user").unwrap(), 1);

    // Re-derive the same content (new UUID, same hash). With dedup it
    // should be suppressed.
    let obs2 = make_observation(
        "User prefers dark mode",
        ObservationLevel::Explicit,
        vec![],
        None,
    );
    let second = insert_observation_dedup(&store, &obs2, None).unwrap();
    assert!(!second, "duplicate content should be suppressed");
    assert_eq!(count_observations(&store, "user").unwrap(), 1);
}

#[test]
fn concurrent_inserts_race_safe() {
    use std::sync::{Arc, Mutex};
    use std::thread;

    // Production wraps the store in a Mutex/RwLock; mirror that here so we
    // exercise the realistic locking contract. The hot cache (in dedup.rs)
    // has its own internal mutex which is what arbitrates the actual race.
    let (store, _dir, _guard) = open_store();
    let store = Arc::new(Mutex::new(store));
    let h = "concurrent-hash";

    let mut handles = Vec::new();
    for _ in 0..20 {
        let s = store.clone();
        let hh = h.to_string();
        handles.push(thread::spawn(move || {
            let guard = s.lock().unwrap();
            is_duplicate(
                &guard,
                &hh,
                DedupKind::Observation,
                Duration::from_secs(300),
            )
        }));
    }
    let mut dup_count = 0usize;
    let mut insert_count = 0usize;
    for h in handles {
        if h.join().unwrap() {
            dup_count += 1;
        } else {
            insert_count += 1;
        }
    }
    // Exactly one thread wins the initial insert; the rest see duplicates.
    assert_eq!(insert_count, 1);
    assert_eq!(dup_count, 19);
}
