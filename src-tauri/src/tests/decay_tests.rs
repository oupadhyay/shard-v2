//! Phase 1.3 — Observation decay + eviction tests.

use crate::observations::{
    count_observations, decay_score, decay_sweep, hard_delete_expired, insert_observation,
    make_observation, recompute_decay, touch_observation, ObservationLevel,
    DECAY_HALF_LIFE_DAYS, DEFAULT_EVICT_THRESHOLD,
};
use crate::vector_store::VectorStore;
use rusqlite::params;
use tempfile::tempdir;

fn open_store() -> (VectorStore, tempfile::TempDir) {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("decay.sqlite");
    let store = VectorStore::open(&db_path).unwrap();
    (store, dir)
}

#[test]
fn fresh_observation_score_is_one() {
    assert!((decay_score(0.0, 0) - 1.0).abs() < 1e-6);
}

#[test]
fn score_halves_after_one_half_life() {
    // No reinforcement; should hit ~0.5 at the half-life.
    let s = decay_score(DECAY_HALF_LIFE_DAYS, 0);
    // The boost factor is 1.0 + ln(1)*0.15 = 1.0 with zero derivations, so
    // the curve is exactly 0.5 at the half-life.
    assert!((s - 0.5).abs() < 1e-3, "expected ~0.5, got {}", s);
}

#[test]
fn score_at_5_half_lives_is_below_threshold() {
    // After 5 half-lives the score is 1/32 ≈ 0.031, comfortably below the
    // 0.05 evict threshold. (4 half-lives lands at 0.0625, just above.)
    let s = decay_score(DECAY_HALF_LIFE_DAYS * 5.0, 0);
    assert!(s < DEFAULT_EVICT_THRESHOLD, "expected <0.05, got {}", s);
}

#[test]
fn times_derived_boosts_score() {
    let cold = decay_score(DECAY_HALF_LIFE_DAYS, 0);
    let hot = decay_score(DECAY_HALF_LIFE_DAYS, 10);
    assert!(hot > cold, "derivation boost should raise the score");
    // But never above 1.0.
    assert!(hot <= 1.0);
}

#[test]
fn touch_observation_reinforces_existing_row() {
    let (store, _dir) = open_store();
    let obs = make_observation("X", ObservationLevel::Explicit, vec![], None);
    let id = obs.id.clone();
    insert_observation(&store, &obs, None).unwrap();

    // Manually push the score low so we can verify reinforcement.
    store
        .conn
        .execute(
            "UPDATE observations SET decay_score = 0.5 WHERE id = ?",
            params![id],
        )
        .unwrap();

    let new_score = touch_observation(&store, &id).unwrap();
    assert!(matches!(new_score, Some(s) if s > 0.5 && s <= 1.0));
}

#[test]
fn touch_observation_returns_none_for_missing_id() {
    let (store, _dir) = open_store();
    let none = touch_observation(&store, "does-not-exist").unwrap();
    assert!(none.is_none());
}

#[test]
fn recompute_decay_writes_score_for_all_live_rows() {
    let (store, _dir) = open_store();
    for i in 0..5 {
        let obs = make_observation(
            &format!("fact {}", i),
            ObservationLevel::Explicit,
            vec![],
            None,
        );
        insert_observation(&store, &obs, None).unwrap();
    }
    let updated = recompute_decay(&store).unwrap();
    assert_eq!(updated, 5);

    // All scores should be present and in [0,1].
    let scores: Vec<f64> = store
        .conn
        .prepare("SELECT decay_score FROM observations")
        .unwrap()
        .query_map([], |row| row.get::<_, f64>(0))
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    assert_eq!(scores.len(), 5);
    for s in scores {
        assert!((0.0..=1.0).contains(&s));
    }
}

#[test]
fn decay_sweep_marks_low_score_rows_deleted() {
    let (store, _dir) = open_store();
    let keep = make_observation("keep", ObservationLevel::Explicit, vec![], None);
    let drop = make_observation("drop", ObservationLevel::Explicit, vec![], None);
    let drop_id = drop.id.clone();
    insert_observation(&store, &keep, None).unwrap();
    insert_observation(&store, &drop, None).unwrap();

    // Force "drop" below the threshold; leave "keep" at the default 1.0.
    store
        .conn
        .execute(
            "UPDATE observations SET decay_score = 0.01 WHERE id = ?",
            params![drop_id],
        )
        .unwrap();

    let evicted = decay_sweep(&store, DEFAULT_EVICT_THRESHOLD).unwrap();
    assert_eq!(evicted, 1);
    assert_eq!(count_observations(&store, "user").unwrap(), 1);

    // The drop row should be soft-deleted (deleted_at set), not hard-removed.
    let deleted_at: Option<String> = store
        .conn
        .query_row(
            "SELECT deleted_at FROM observations WHERE id = ?",
            params![drop_id],
            |row| row.get(0),
        )
        .unwrap();
    assert!(deleted_at.is_some());
}

#[test]
fn touched_observations_survive_sweep() {
    let (store, _dir) = open_store();
    let obs = make_observation("hot", ObservationLevel::Explicit, vec![], None);
    let id = obs.id.clone();
    insert_observation(&store, &obs, None).unwrap();

    // Drive score very low manually...
    store
        .conn
        .execute(
            "UPDATE observations SET decay_score = 0.01 WHERE id = ?",
            params![id],
        )
        .unwrap();
    // ...then touch (reinforcement boost should push it above threshold).
    let restored = touch_observation(&store, &id).unwrap().unwrap();
    assert!(restored > DEFAULT_EVICT_THRESHOLD);

    let evicted = decay_sweep(&store, DEFAULT_EVICT_THRESHOLD).unwrap();
    assert_eq!(evicted, 0);
}

#[test]
fn hard_delete_after_grace_period() {
    let (store, _dir) = open_store();
    let obs_old = make_observation("old", ObservationLevel::Explicit, vec![], None);
    let obs_new = make_observation("new", ObservationLevel::Explicit, vec![], None);
    let old_id = obs_old.id.clone();
    let new_id = obs_new.id.clone();
    insert_observation(&store, &obs_old, None).unwrap();
    insert_observation(&store, &obs_new, None).unwrap();

    // Soft-delete both with different ages.
    let long_ago = (chrono::Utc::now() - chrono::Duration::days(100)).to_rfc3339();
    let recently = chrono::Utc::now().to_rfc3339();
    store
        .conn
        .execute(
            "UPDATE observations SET deleted_at = ? WHERE id = ?",
            params![long_ago, old_id],
        )
        .unwrap();
    store
        .conn
        .execute(
            "UPDATE observations SET deleted_at = ? WHERE id = ?",
            params![recently, new_id],
        )
        .unwrap();

    // Grace = 90 days. The old row falls outside; the new one stays.
    let removed = hard_delete_expired(&store, chrono::Duration::days(90)).unwrap();
    assert_eq!(removed, 1);

    let still_there: i64 = store
        .conn
        .query_row(
            "SELECT COUNT(*) FROM observations WHERE id = ?",
            params![new_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(still_there, 1);
    let gone: i64 = store
        .conn
        .query_row(
            "SELECT COUNT(*) FROM observations WHERE id = ?",
            params![old_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(gone, 0);
}
