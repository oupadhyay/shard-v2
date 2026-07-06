use crate::vector_store::VectorStore;
use rusqlite::OptionalExtension;
use tempfile::tempdir;

fn open_test_store() -> (VectorStore, tempfile::TempDir) {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("test.sqlite");
    let store = VectorStore::open(&db_path).unwrap();
    (store, dir)
}

#[test]
fn test_chunks_table_has_session_constraint() {
    let (store, _dir) = open_test_store();

    let sql: String = store
        .conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name='chunks'",
            [],
            |row| row.get(0),
        )
        .unwrap();

    assert!(
        sql.contains("session"),
        "chunks CHECK constraint should include 'session', got: {}",
        sql
    );
    assert!(sql.contains("topic"), "chunks CHECK should include 'topic'");
    assert!(
        sql.contains("insight"),
        "chunks CHECK should include 'insight'"
    );
}

#[test]
fn test_sessions_table_has_active_skills() {
    let (store, _dir) = open_test_store();

    let mut found = false;
    let mut stmt = store.conn.prepare("PRAGMA table_info(sessions)").unwrap();
    let rows = stmt
        .query_map([], |row| {
            let name: String = row.get(1)?;
            Ok(name)
        })
        .unwrap();

    for row in rows {
        if row.unwrap() == "active_skills" {
            found = true;
            break;
        }
    }

    assert!(found, "sessions table should have 'active_skills' column");
}

#[test]
fn test_observation_tables_exist() {
    let (store, _dir) = open_test_store();

    let tables = vec![
        "observations",
        "observation_embeddings",
        "observations_fts",
        "peer_cards",
    ];

    for table in tables {
        let exists: bool = store
            .conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE name = ?1",
                [table],
                |row| row.get(0),
            )
            .unwrap();
        assert!(exists, "table '{}' should exist", table);
    }
}

#[test]
fn test_fts_triggers_exist() {
    let (store, _dir) = open_test_store();

    let triggers = vec![
        "chunks_ai",
        "chunks_ad",
        "chunks_au",
        "obs_fts_ai",
        "obs_fts_ad",
        "obs_fts_au",
    ];

    for trigger in triggers {
        let exists: bool = store
            .conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='trigger' AND name = ?1",
                [trigger],
                |row| row.get(0),
            )
            .unwrap();
        assert!(exists, "trigger '{}' should exist", trigger);
    }
}

#[test]
fn test_session_migration_flag() {
    let (store, _dir) = open_test_store();

    // After removing the migration flag INSERT from schema.sql,
    // a fresh DB should NOT have the session_migration_completed key at all.
    let result: Option<String> = store
        .conn
        .query_row(
            "SELECT value FROM metadata WHERE key = 'session_migration_completed'",
            [],
            |row| row.get(0),
        )
        .optional()
        .unwrap();

    assert!(
        result.is_none(),
        "Fresh DB should not have session_migration_completed flag"
    );
}

#[test]
fn test_fresh_db_has_complete_schema() {
    let (store, _dir) = open_test_store();

    let expected_tables = vec![
        "chunks",
        "chunk_embeddings",
        "embedding_cache",
        "metadata",
        "chunks_fts",
        "sessions",
        "messages",
        "observations",
        "observation_embeddings",
        "observations_fts",
        "peer_cards",
    ];

    for table in &expected_tables {
        let exists: bool = store
            .conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE name = ?1",
                [table],
                |row| row.get(0),
            )
            .unwrap();
        assert!(exists, "table '{}' should exist in fresh DB", table);
    }

    let expected_indexes = vec![
        "idx_chunks_source",
        "idx_chunks_hash",
        "idx_obs_observed",
        "idx_obs_observer",
        "idx_obs_session",
        "idx_obs_hash",
    ];

    for index in &expected_indexes {
        let exists: bool = store
            .conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='index' AND name = ?1",
                [index],
                |row| row.get(0),
            )
            .unwrap();
        assert!(exists, "index '{}' should exist in fresh DB", index);
    }

    let expected_triggers = vec![
        "chunks_ai",
        "chunks_ad",
        "chunks_au",
        "obs_fts_ai",
        "obs_fts_ad",
        "obs_fts_au",
    ];

    for trigger in &expected_triggers {
        let exists: bool = store
            .conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='trigger' AND name = ?1",
                [trigger],
                |row| row.get(0),
            )
            .unwrap();
        assert!(exists, "trigger '{}' should exist in fresh DB", trigger);
    }
}
