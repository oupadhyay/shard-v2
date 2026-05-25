//! Phase 2.2 — typed observation edges + temporal validity tests.

use crate::observations::{
    causal_chain, currently_valid, insert_observation, insert_with_edge, make_observation,
    supersede, EdgeKind, ObservationLevel,
};
use crate::vector_store::VectorStore;
use rusqlite::params;
use tempfile::tempdir;

fn open_store() -> (VectorStore, tempfile::TempDir) {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("typed_edges.sqlite");
    let store = VectorStore::open(&db_path).unwrap();
    (store, dir)
}

#[test]
fn edge_kind_roundtrips_through_string() {
    for k in &[
        EdgeKind::Derived,
        EdgeKind::Modifies,
        EdgeKind::Causes,
        EdgeKind::Fixes,
        EdgeKind::Contradicts,
        EdgeKind::DependsOn,
        EdgeKind::Uses,
    ] {
        let s = k.as_str();
        assert_eq!(EdgeKind::parse(s), Some(*k));
    }
    assert!(EdgeKind::parse("bogus").is_none());
}

#[test]
fn insert_with_edge_persists_kind() {
    let (store, _dir) = open_store();
    let parent = make_observation("user lives in SF", ObservationLevel::Explicit, vec![], None);
    insert_observation(&store, &parent, None).unwrap();

    let child = make_observation(
        "user commutes via BART",
        ObservationLevel::Deductive,
        vec![parent.id.clone()],
        None,
    );
    insert_with_edge(&store, &child, None, EdgeKind::Causes).unwrap();

    let stored: String = store
        .conn
        .query_row(
            "SELECT edge_kind FROM observations WHERE id = ?",
            params![child.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stored, "causes");
}

#[test]
fn supersede_closes_tvalid_end_and_links_contradiction() {
    let (store, _dir) = open_store();
    let old = make_observation(
        "user prefers Gemini",
        ObservationLevel::Explicit,
        vec![],
        None,
    );
    insert_observation(&store, &old, None).unwrap();

    let mut new = make_observation(
        "user prefers OpenRouter",
        ObservationLevel::Explicit,
        vec![],
        None,
    );
    let new_id = new.id.clone();
    supersede(&store, &old.id, &mut new, None).unwrap();

    // Old row now has tvalid_end set.
    let tvalid_end: Option<String> = store
        .conn
        .query_row(
            "SELECT tvalid_end FROM observations WHERE id = ?",
            params![old.id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(tvalid_end.is_some());

    // New row has Contradicts edge + source_ids contains the old id.
    let (edge, src): (String, String) = store
        .conn
        .query_row(
            "SELECT edge_kind, source_ids FROM observations WHERE id = ?",
            params![new_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(edge, "contradicts");
    let src_vec: Vec<String> = serde_json::from_str(&src).unwrap();
    assert!(src_vec.iter().any(|s| s == &old.id));
}

#[test]
fn currently_valid_excludes_superseded() {
    let (store, _dir) = open_store();
    let old = make_observation("ephemeral", ObservationLevel::Explicit, vec![], None);
    insert_observation(&store, &old, None).unwrap();
    let stable = make_observation("stable", ObservationLevel::Explicit, vec![], None);
    insert_observation(&store, &stable, None).unwrap();

    let mut new = make_observation("ephemeral v2", ObservationLevel::Explicit, vec![], None);
    supersede(&store, &old.id, &mut new, None).unwrap();

    let live = currently_valid(&store, "user", 20).unwrap();
    let ids: Vec<&str> = live.iter().map(|o| o.id.as_str()).collect();
    assert!(ids.contains(&stable.id.as_str()));
    assert!(ids.contains(&new.id.as_str()));
    assert!(
        !ids.contains(&old.id.as_str()),
        "superseded row should not appear in currently_valid"
    );
}

#[test]
fn currently_valid_includes_legacy_rows() {
    let (store, _dir) = open_store();
    // Insert an observation, then simulate a legacy row by clearing
    // tvalid_start (Phase 2.2 migration backfilled this; pre-migration
    // rows would have been NULL).
    let obs = make_observation("legacy", ObservationLevel::Explicit, vec![], None);
    insert_observation(&store, &obs, None).unwrap();
    store
        .conn
        .execute(
            "UPDATE observations SET tvalid_start = NULL WHERE id = ?",
            params![obs.id],
        )
        .unwrap();
    let live = currently_valid(&store, "user", 5).unwrap();
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].id, obs.id);
}

#[test]
fn causal_chain_walks_ancestors_in_bfs_order() {
    let (store, _dir) = open_store();
    // A → B → C (C derived from B, B derived from A)
    let a = make_observation("A", ObservationLevel::Explicit, vec![], None);
    insert_observation(&store, &a, None).unwrap();
    let b = make_observation("B", ObservationLevel::Deductive, vec![a.id.clone()], None);
    insert_observation(&store, &b, None).unwrap();
    let c = make_observation("C", ObservationLevel::Inductive, vec![b.id.clone()], None);
    insert_observation(&store, &c, None).unwrap();

    let chain = causal_chain(&store, &c.id, 5).unwrap();
    let ids: Vec<&str> = chain.iter().map(|o| o.id.as_str()).collect();
    // BFS visits B before A (B is depth 1, A is depth 2).
    assert_eq!(ids, vec![b.id.as_str(), a.id.as_str()]);
    // The seed itself is excluded from the output.
    assert!(!ids.contains(&c.id.as_str()));
}

#[test]
fn causal_chain_respects_depth_cap() {
    let (store, _dir) = open_store();
    let a = make_observation("A", ObservationLevel::Explicit, vec![], None);
    insert_observation(&store, &a, None).unwrap();
    let b = make_observation("B", ObservationLevel::Deductive, vec![a.id.clone()], None);
    insert_observation(&store, &b, None).unwrap();
    let c = make_observation("C", ObservationLevel::Inductive, vec![b.id.clone()], None);
    insert_observation(&store, &c, None).unwrap();

    // depth=1 only reaches B (the direct parent of C).
    let chain = causal_chain(&store, &c.id, 1).unwrap();
    let ids: Vec<&str> = chain.iter().map(|o| o.id.as_str()).collect();
    assert_eq!(ids, vec![b.id.as_str()]);
}

#[test]
fn causal_chain_does_not_loop_on_cycles() {
    let (store, _dir) = open_store();
    // Pathological self-cycle: insert A, then mutate its source_ids in SQL
    // to include itself. causal_chain must terminate.
    let a = make_observation("A", ObservationLevel::Explicit, vec![], None);
    insert_observation(&store, &a, None).unwrap();
    store
        .conn
        .execute(
            "UPDATE observations SET source_ids = ? WHERE id = ?",
            params![serde_json::json!([&a.id]).to_string(), a.id],
        )
        .unwrap();
    // Should return empty (the seed itself is excluded, and the only ancestor
    // is the seed) and definitely not hang.
    let chain = causal_chain(&store, &a.id, 5).unwrap();
    assert!(chain.is_empty());
}
