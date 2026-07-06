/**
 * Vector Store module - SQLite-backed embedding storage with sqlite-vec
 *
 * Provides persistent vector similarity search using sqlite-vec extension.
 * Replaces JSON-based chunk index with ACID-compliant SQLite storage.
 */
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use std::path::Path;

use crate::memories::{Chunk, ChunkIndex, SourceType};

// ============================================================================
// Error Type
// ============================================================================

#[derive(Debug)]
pub enum VectorStoreError {
    Sqlite(rusqlite::Error),
    SqliteVec(String),
    Migration(String),
    Io(std::io::Error),
}

impl std::fmt::Display for VectorStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VectorStoreError::Sqlite(e) => write!(f, "SQLite error: {}", e),
            VectorStoreError::SqliteVec(e) => write!(f, "sqlite-vec error: {}", e),
            VectorStoreError::Migration(e) => write!(f, "Migration error: {}", e),
            VectorStoreError::Io(e) => write!(f, "IO error: {}", e),
        }
    }
}

impl std::error::Error for VectorStoreError {}

impl From<rusqlite::Error> for VectorStoreError {
    fn from(e: rusqlite::Error) -> Self {
        VectorStoreError::Sqlite(e)
    }
}

impl From<std::io::Error> for VectorStoreError {
    fn from(e: std::io::Error) -> Self {
        VectorStoreError::Io(e)
    }
}

// ============================================================================
// VectorStore
// ============================================================================

/// SQLite-backed vector store using sqlite-vec for KNN search
pub struct VectorStore {
    pub(crate) conn: Connection,
}

/// Embedding dimension (Gemini Embedding 2 with output_dimensionality=768)
const EMBEDDING_DIM: usize = 768;

impl VectorStore {
    /// Open or create a vector store at the given path
    pub fn open(db_path: &Path) -> Result<Self, VectorStoreError> {
        // Ensure parent directory exists
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        use std::sync::OnceLock;
        static VEC_INIT_STATUS: OnceLock<i32> = OnceLock::new();

        let init_rc = *VEC_INIT_STATUS.get_or_init(|| unsafe {
            rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute::<
                *const (),
                unsafe extern "C" fn(
                    *mut rusqlite::ffi::sqlite3,
                    *mut *const i8,
                    *const rusqlite::ffi::sqlite3_api_routines,
                ) -> i32,
            >(
                sqlite_vec::sqlite3_vec_init as *const ()
            )))
        });

        if init_rc != 0 {
            return Err(VectorStoreError::SqliteVec(format!(
                "sqlite3_auto_extension failed with code {}",
                init_rc
            )));
        }

        let conn = Connection::open(db_path)?;

        // Enable WAL mode for better concurrency
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;

        // Initialize schema
        conn.execute_batch(include_str!("schema.sql"))?;

        // Phase 1.3 additive migrations: add decay columns to `observations` if
        // they aren't already present. `ALTER TABLE ... ADD COLUMN` errors if the
        // column exists, so we guard with `pragma_table_info`.
        Self::ensure_column(&conn, "observations", "last_accessed", "TEXT")?;
        Self::ensure_column(
            &conn,
            "observations",
            "decay_score",
            "REAL NOT NULL DEFAULT 1.0",
        )?;
        // Backfill last_accessed for legacy rows on first run.
        conn.execute(
            "UPDATE observations SET last_accessed = created_at WHERE last_accessed IS NULL",
            [],
        )?;
        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_obs_decay ON observations(decay_score, deleted_at);",
        )?;

        // Phase 2.3: snapshot-for-rollback column on file_events.
        Self::ensure_column(&conn, "file_events", "before_content", "TEXT")?;

        // Phase 2.2 additive migrations: typed edges + temporal validity.
        // `edge_kind` is nullable — NULL means legacy 'derived' edge so old
        // queries keep working. `tvalid_start`/`tvalid_end` enable
        // supersede/causal-chain queries.
        Self::ensure_column(&conn, "observations", "edge_kind", "TEXT")?;
        Self::ensure_column(&conn, "observations", "tvalid_start", "TEXT")?;
        Self::ensure_column(&conn, "observations", "tvalid_end", "TEXT")?;
        // Backfill: legacy rows are valid starting from when they were
        // created, with no end.
        conn.execute(
            "UPDATE observations SET tvalid_start = created_at WHERE tvalid_start IS NULL",
            [],
        )?;
        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_obs_edge ON observations(edge_kind);\n\
             CREATE INDEX IF NOT EXISTS idx_obs_tvalid ON observations(tvalid_end);",
        )?;

        // Phase 3.2: persist a per-sketch crystallisation timestamp so the
        // background sweep doesn't reprocess the same recipe every 6h.
        Self::ensure_column(&conn, "actions", "crystallized_at", "TEXT")?;

        // Phase 1.3: drop+recreate the obs_fts_au trigger so existing
        // databases pick up the narrowed `AFTER UPDATE OF content` scope.
        // Without this, every decay_score UPDATE triggers a full FTS5 row
        // rewrite — turning a 10k-row sweep into an 11-second job. The
        // CREATE here mirrors schema.sql exactly.
        conn.execute_batch(
            "DROP TRIGGER IF EXISTS obs_fts_au;\n\
             CREATE TRIGGER IF NOT EXISTS obs_fts_au \
                AFTER UPDATE OF content ON observations BEGIN \
                  UPDATE observations_fts SET content = new.content \
                  WHERE observation_id = old.id; \
                END;",
        )?;

        log::info!("[VectorStore] Opened database at {:?}", db_path);

        Ok(Self { conn })
    }

    /// Idempotent `ALTER TABLE ... ADD COLUMN`. Skips if the column already
    /// exists. Used for additive Phase 1+ migrations on the `observations`
    /// table without requiring a full migration framework.
    fn ensure_column(
        conn: &Connection,
        table: &str,
        column: &str,
        decl: &str,
    ) -> Result<(), VectorStoreError> {
        let mut stmt = conn.prepare(&format!("PRAGMA table_info({})", table))?;
        let existing: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(1))?
            .filter_map(|r| r.ok())
            .collect();
        if existing.iter().any(|c| c == column) {
            return Ok(());
        }
        conn.execute(
            &format!("ALTER TABLE {} ADD COLUMN {} {}", table, column, decl),
            [],
        )?;
        Ok(())
    }

    /// Execute a closure within a single SQLite transaction.
    /// The transaction is committed on success and rolled back on error.
    pub fn with_transaction<F, T>(&self, f: F) -> Result<T, VectorStoreError>
    where
        F: FnOnce(&Self, &Connection) -> Result<T, VectorStoreError>,
    {
        let tx = self.conn.unchecked_transaction()?;
        let result = f(self, &tx)?;
        tx.commit()?;
        Ok(result)
    }

    /// Get the number of chunks in the store
    pub fn chunk_count(&self) -> Result<usize, VectorStoreError> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM chunks", [], |row| row.get(0))?;
        Ok(count as usize)
    }

    /// Get an embedding from cache by content hash
    pub fn get_cached_embedding(
        &self,
        content_hash: &str,
    ) -> Result<Option<Vec<f32>>, VectorStoreError> {
        let result: Option<Vec<u8>> = self
            .conn
            .query_row(
                "SELECT embedding FROM embedding_cache WHERE content_hash = ?",
                [content_hash],
                |row| row.get(0),
            )
            .optional()?;

        match result {
            Some(bytes) => Ok(Some(bytes_to_f32_vec(&bytes)?)),
            None => Ok(None),
        }
    }

    /// Cache an embedding for future reuse
    pub fn cache_embedding(
        &self,
        content_hash: &str,
        embedding: &[f32],
    ) -> Result<(), VectorStoreError> {
        // Validate dimension before caching to prevent cache poisoning
        if embedding.len() != EMBEDDING_DIM {
            return Err(VectorStoreError::Migration(format!(
                "Cannot cache embedding with wrong dimension: expected {}, got {}",
                EMBEDDING_DIM,
                embedding.len()
            )));
        }

        let embedding_bytes = f32_vec_to_bytes(embedding);
        let now = Utc::now().to_rfc3339();

        self.conn.execute(
            "INSERT OR REPLACE INTO embedding_cache (content_hash, embedding, created_at) VALUES (?, ?, ?)",
            params![content_hash, embedding_bytes, now],
        )?;

        Ok(())
    }

    /// Insert or update a chunk with its embedding (public wrapper with transaction)
    pub fn upsert_chunk(&self, chunk: &Chunk) -> Result<(), VectorStoreError> {
        let tx = self.conn.unchecked_transaction()?;
        self.upsert_chunk_internal(&tx, chunk)?;
        tx.commit()?;
        Ok(())
    }

    /// Internal logic for upserting a chunk, using an existing connection/transaction
    pub(crate) fn upsert_chunk_internal(
        &self,
        conn: &Connection,
        chunk: &Chunk,
    ) -> Result<(), VectorStoreError> {
        let now = Utc::now().to_rfc3339();
        let source_type_str = match chunk.source_type {
            SourceType::Topic => "topic",
            SourceType::Insight => "insight",
            SourceType::Session => "session",
        };
        let content_hash = compute_content_hash(&chunk.text);

        if chunk.embedding.len() != EMBEDDING_DIM {
            return Err(VectorStoreError::Migration(format!(
                "Invalid embedding dimension: expected {}, got {}",
                EMBEDDING_DIM,
                chunk.embedding.len()
            )));
        }

        // Upsert metadata
        conn.execute(
            r#"
            INSERT INTO chunks (id, source_type, source_name, heading, text, start_line, end_line, content_hash, created_at, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)
            ON CONFLICT(id) DO UPDATE SET
                source_type = excluded.source_type,
                source_name = excluded.source_name,
                heading = excluded.heading,
                text = excluded.text,
                start_line = excluded.start_line,
                end_line = excluded.end_line,
                content_hash = excluded.content_hash,
                updated_at = excluded.updated_at
            "#,
            params![
                chunk.id,
                source_type_str,
                chunk.source_name,
                chunk.heading,
                chunk.text,
                chunk.start_line,
                chunk.end_line,
                content_hash,
                now,
            ],
        )?;

        // Upsert embedding into vec0 virtual table
        // vec0 doesn't strictly support UPSERT/REPLACE in all versions, so we DELETE then INSERT
        let embedding_bytes = f32_vec_to_bytes(&chunk.embedding);

        conn.execute(
            "DELETE FROM chunk_embeddings WHERE chunk_id = ?",
            [chunk.id.as_str()],
        )?;

        conn.execute(
            "INSERT INTO chunk_embeddings (chunk_id, embedding) VALUES (?, ?)",
            params![chunk.id, embedding_bytes],
        )?;

        // Also cache the embedding
        conn.execute(
            "INSERT OR REPLACE INTO embedding_cache (content_hash, embedding, created_at) VALUES (?, ?, ?)",
            params![content_hash, embedding_bytes, now],
        )?;

        Ok(())
    }
    /// Delete all chunks for a given source (public wrapper with transaction)
    pub fn delete_by_source(
        &self,
        source_type: SourceType,
        source_name: &str,
    ) -> Result<usize, VectorStoreError> {
        let tx = self.conn.unchecked_transaction()?;
        let deleted = self.delete_by_source_internal(&tx, source_type, source_name)?;
        tx.commit()?;
        Ok(deleted)
    }

    /// Internal logic for deleting chunks by source
    pub(crate) fn delete_by_source_internal(
        &self,
        conn: &Connection,
        source_type: SourceType,
        source_name: &str,
    ) -> Result<usize, VectorStoreError> {
        let source_type_str = match source_type {
            SourceType::Topic => "topic",
            SourceType::Insight => "insight",
            SourceType::Session => "session",
        };

        // Delete from vec0 table using subquery (efficient single-statement delete)
        conn.execute(
            "DELETE FROM chunk_embeddings WHERE chunk_id IN (SELECT id FROM chunks WHERE source_type = ? AND source_name = ?)",
            params![source_type_str, source_name],
        )?;

        // Delete from chunks table
        let deleted = conn.execute(
            "DELETE FROM chunks WHERE source_type = ? AND source_name = ?",
            params![source_type_str, source_name],
        )?;

        Ok(deleted)
    }
    /// Returns top-k chunks ordered by cosine similarity (descending)
    pub fn knn_search(
        &self,
        query_embedding: &[f32],
        k: usize,
        min_score: f32,
    ) -> Result<Vec<Chunk>, VectorStoreError> {
        // Validate embedding dimension before passing to SQLite
        if query_embedding.len() != EMBEDDING_DIM {
            return Err(VectorStoreError::Migration(format!(
                "Query embedding dimension mismatch: expected {}, got {}",
                EMBEDDING_DIM,
                query_embedding.len()
            )));
        }

        let query_bytes = f32_vec_to_bytes(query_embedding);

        // sqlite-vec optimized KNN query: use 'MATCH' operator
        // This avoids full table scan by using the virtual table's index mechanism
        let mut stmt = self.conn.prepare(
            r#"
            SELECT
                c.id, c.source_type, c.source_name, c.heading, c.text,
                c.start_line, c.end_line,
                e.embedding,
                (1.0 - e.distance) as similarity
            FROM chunk_embeddings e
            JOIN chunks c ON c.id = e.chunk_id
            WHERE e.embedding MATCH ?1 AND k = ?2
            ORDER BY e.distance
            "#,
        )?;

        // Filter min_score in application code
        let rows = stmt.query_map(params![query_bytes, k as i64], |row| {
            let source_type_str: String = row.get(1)?;
            let embedding_bytes: Vec<u8> = row.get(7)?;
            let similarity: f32 = row.get(8)?;

            let source_type = match source_type_str.as_str() {
                "topic" => SourceType::Topic,
                "insight" => SourceType::Insight,
                "session" => SourceType::Session,
                other => {
                    return Err(rusqlite::Error::FromSqlConversionFailure(
                        1,
                        rusqlite::types::Type::Text,
                        Box::new(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            format!("invalid source_type value: {}", other),
                        )),
                    ));
                }
            };

            Ok((
                Chunk {
                    id: row.get(0)?,
                    source_type,
                    source_name: row.get(2)?,
                    heading: row.get(3)?,
                    text: row.get(4)?,
                    start_line: row.get(5)?,
                    end_line: row.get(6)?,
                    embedding: bytes_to_f32_vec(&embedding_bytes).map_err(|e| {
                        rusqlite::Error::InvalidColumnType(
                            7,
                            e.to_string(),
                            rusqlite::types::Type::Blob,
                        )
                    })?,
                },
                similarity,
            ))
        })?;

        // Collect with error propagation instead of silently dropping errors
        let mut results: Vec<Chunk> = Vec::new();
        for row_result in rows {
            let (chunk, score) = row_result?;
            if score >= min_score {
                results.push(chunk);
            }
        }

        Ok(results)
    }

    /// Search using FTS5 (Keyword Search)
    pub fn search_fts(
        &self,
        query_text: &str,
        limit: usize,
    ) -> Result<Vec<Chunk>, VectorStoreError> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT
                c.id, c.source_type, c.source_name, c.heading, c.text,
                c.start_line, c.end_line,
                e.embedding
            FROM chunks_fts
            JOIN chunks c ON c.id = chunks_fts.chunk_id
            JOIN chunk_embeddings e ON c.id = e.chunk_id
            WHERE chunks_fts MATCH ?1
            ORDER BY bm25(chunks_fts)
            LIMIT ?2
            "#,
        )?;

        let params = params![query_text, limit as i64];
        let chunks = stmt.query_map(params, |row| {
            let source_type_str: String = row.get(1)?;
            let embedding_bytes: Vec<u8> = row.get(7)?;

            let source_type = match source_type_str.as_str() {
                "topic" => SourceType::Topic,
                "insight" => SourceType::Insight,
                "session" => SourceType::Session,
                _ => SourceType::Insight, // Fallback for FTS results if type unknown
            };

            Ok(Chunk {
                id: row.get(0)?,
                source_type,
                source_name: row.get(2)?,
                heading: row.get(3)?,
                text: row.get(4)?,
                start_line: row.get(5)?,
                end_line: row.get(6)?,
                embedding: bytes_to_f32_vec(&embedding_bytes).map_err(|e| {
                    rusqlite::Error::InvalidColumnType(
                        7,
                        e.to_string(),
                        rusqlite::types::Type::Blob,
                    )
                })?,
            })
        })?;

        // Collect with error propagation instead of silently dropping errors
        chunks
            .collect::<Result<Vec<_>, _>>()
            .map_err(VectorStoreError::from)
    }

    /// Hybrid Search (Vector + Keyword)
    /// Returns a merged list of chunks, deduplicated by ID.
    /// vector_min_score: Threshold for vector search (e.g. 0.4)
    pub fn hybrid_search(
        &self,
        query_text: &str,
        query_embedding: &[f32],
        k: usize,
        vector_min_score: f32,
    ) -> Result<Vec<Chunk>, VectorStoreError> {
        use std::collections::HashSet;

        // 1. Get Top-K Vector Results (filtered)
        let vec_results = self.knn_search(query_embedding, k, vector_min_score)?;

        // 2. Get Top-K Keyword Results
        let fts_results = self.search_fts(query_text, k)?;

        // 3. Merge (Union)
        let mut merged = vec_results;
        let mut seen_ids: HashSet<String> = merged.iter().map(|c| c.id.clone()).collect();

        for chunk in fts_results {
            if !seen_ids.contains(&chunk.id) {
                seen_ids.insert(chunk.id.clone());
                merged.push(chunk);
            }
        }

        Ok(merged)
    }

    /// Get all chunks (for migration/debugging)
    pub fn get_all_chunks(&self) -> Result<Vec<Chunk>, VectorStoreError> {
        let mut stmt = self.conn.prepare(
            r#"
            SELECT c.id, c.source_type, c.source_name, c.heading, c.text,
                   c.start_line, c.end_line, e.embedding
            FROM chunks c
            JOIN chunk_embeddings e ON c.id = e.chunk_id
            "#,
        )?;

        let chunks = stmt.query_map([], |row| {
            let source_type_str: String = row.get(1)?;
            let embedding_bytes: Vec<u8> = row.get(7)?;

            let source_type = match source_type_str.as_str() {
                "topic" => SourceType::Topic,
                "insight" => SourceType::Insight,
                "session" => SourceType::Session,
                _ => SourceType::Insight,
            };

            Ok(Chunk {
                id: row.get(0)?,
                source_type,
                source_name: row.get(2)?,
                heading: row.get(3)?,
                text: row.get(4)?,
                start_line: row.get(5)?,
                end_line: row.get(6)?,
                embedding: bytes_to_f32_vec(&embedding_bytes).map_err(|e| {
                    rusqlite::Error::InvalidColumnType(
                        7,
                        e.to_string(),
                        rusqlite::types::Type::Blob,
                    )
                })?,
            })
        })?;

        // Collect with error propagation instead of silently dropping errors
        chunks
            .collect::<Result<Vec<_>, _>>()
            .map_err(VectorStoreError::from)
    }

    /// Get all unique sources (type, name) in the store
    pub fn get_unique_sources(&self) -> Result<Vec<(SourceType, String)>, VectorStoreError> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT source_type, source_name FROM chunks")?;

        let rows = stmt.query_map([], |row| {
            let type_str: String = row.get(0)?;
            let name: String = row.get(1)?;
            let source_type = if type_str == "topic" {
                SourceType::Topic
            } else if type_str == "session" {
                SourceType::Session
            } else {
                SourceType::Insight
            };
            Ok((source_type, name))
        })?;

        // Collect with error propagation instead of silently dropping errors
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(VectorStoreError::from)
    }

    /// Clear the embedding cache (e.g. after switching embedding models)
    pub fn clear_embedding_cache(&self) -> Result<usize, VectorStoreError> {
        let deleted = self.conn.execute("DELETE FROM embedding_cache", [])?;
        log::info!(
            "[VectorStore] Cleared embedding cache ({} entries)",
            deleted
        );
        Ok(deleted)
    }

    /// Clear all data (for testing)
    #[cfg(test)]
    pub fn clear(&self) -> Result<(), VectorStoreError> {
        self.conn.execute_batch(
            "DELETE FROM chunk_embeddings; DELETE FROM chunks; DELETE FROM embedding_cache;",
        )?;
        Ok(())
    }

    /// Migrate from JSON chunk index to SQLite
    pub fn migrate_from_json(&self, chunk_index: &ChunkIndex) -> Result<usize, VectorStoreError> {
        log::info!(
            "[VectorStore] Migrating {} chunks from JSON",
            chunk_index.chunks.len()
        );

        let tx = self.conn.unchecked_transaction()?;
        let mut count = 0;
        for chunk in &chunk_index.chunks {
            self.upsert_chunk_internal(&tx, chunk)?;
            count += 1;
        }
        tx.commit()?;

        log::info!("[VectorStore] Migration complete: {} chunks", count);
        Ok(count)
    }

    /// Get the timestamp of last rebuild (from metadata table)
    pub fn last_rebuilt(&self) -> Result<Option<DateTime<Utc>>, VectorStoreError> {
        let result: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM metadata WHERE key = 'last_rebuilt'",
                [],
                |row| row.get(0),
            )
            .optional()?;

        Ok(result.and_then(|s| {
            DateTime::parse_from_rfc3339(&s)
                .ok()
                .map(|dt| dt.with_timezone(&Utc))
        }))
    }

    /// Set the last rebuilt timestamp
    pub fn set_last_rebuilt(&self, timestamp: DateTime<Utc>) -> Result<(), VectorStoreError> {
        self.conn.execute(
            "INSERT OR REPLACE INTO metadata (key, value) VALUES ('last_rebuilt', ?)",
            [timestamp.to_rfc3339()],
        )?;
        Ok(())
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Compute SHA256 hash of content for cache key
pub fn compute_content_hash(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Convert f32 vector to bytes for SQLite storage
fn f32_vec_to_bytes(v: &[f32]) -> Vec<u8> {
    // Store embeddings in a fixed little-endian representation for portability.
    let mut bytes = Vec::with_capacity(std::mem::size_of_val(v));
    for &value in v {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

/// Convert bytes from SQLite to f32 vector
fn bytes_to_f32_vec(bytes: &[u8]) -> Result<Vec<f32>, VectorStoreError> {
    if !bytes.len().is_multiple_of(std::mem::size_of::<f32>()) {
        return Err(VectorStoreError::Migration(format!(
            "Invalid embedding blob size: {} bytes (not a multiple of 4)",
            bytes.len()
        )));
    }

    let num_f32 = bytes.len() / std::mem::size_of::<f32>();
    if num_f32 != EMBEDDING_DIM {
        return Err(VectorStoreError::Migration(format!(
            "Invalid embedding length: expected {} floats, got {}",
            EMBEDDING_DIM, num_f32
        )));
    }

    Ok(bytes
        .chunks_exact(std::mem::size_of::<f32>())
        .map(|b| {
            let arr: [u8; 4] = b.try_into().unwrap();
            f32::from_le_bytes(arr)
        })
        .collect())
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn make_test_chunk(id: &str, text: &str, source_type: SourceType) -> Chunk {
        Chunk {
            id: id.to_string(),
            source_type,
            source_name: "test_source".to_string(),
            heading: Some("Test Heading".to_string()),
            text: text.to_string(),
            start_line: 1,
            end_line: 10,
            embedding: vec![0.1f32; EMBEDDING_DIM],
        }
    }

    fn make_query_embedding() -> Vec<f32> {
        vec![0.1f32; EMBEDDING_DIM]
    }

    #[test]
    fn test_vector_store_open_creates_db() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.sqlite");

        let store = VectorStore::open(&db_path).unwrap();
        assert!(db_path.exists());
        assert_eq!(store.chunk_count().unwrap(), 0);
    }

    #[test]
    fn test_upsert_chunk_new() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.sqlite");
        let store = VectorStore::open(&db_path).unwrap();

        let chunk = make_test_chunk("chunk_1", "Test content", SourceType::Topic);
        store.upsert_chunk(&chunk).unwrap();

        assert_eq!(store.chunk_count().unwrap(), 1);
    }

    #[test]
    fn test_upsert_chunk_update() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.sqlite");
        let store = VectorStore::open(&db_path).unwrap();

        let chunk1 = make_test_chunk("chunk_1", "Original content", SourceType::Topic);
        store.upsert_chunk(&chunk1).unwrap();

        let mut chunk2 = make_test_chunk("chunk_1", "Updated content", SourceType::Topic);
        chunk2.heading = Some("Updated Heading".to_string());
        store.upsert_chunk(&chunk2).unwrap();

        // Still only 1 chunk
        assert_eq!(store.chunk_count().unwrap(), 1);

        // Verify content was updated
        let chunks = store.get_all_chunks().unwrap();
        assert_eq!(chunks[0].text, "Updated content");
        assert_eq!(chunks[0].heading, Some("Updated Heading".to_string()));
    }

    #[test]
    fn test_knn_search_returns_top_k() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.sqlite");
        let store = VectorStore::open(&db_path).unwrap();

        // Insert 5 chunks
        for i in 0..5 {
            let chunk = make_test_chunk(
                &format!("chunk_{}", i),
                &format!("Content {}", i),
                SourceType::Topic,
            );
            store.upsert_chunk(&chunk).unwrap();
        }

        let query = make_query_embedding();
        let results = store.knn_search(&query, 3, 0.0).unwrap();

        assert_eq!(results.len(), 3);
    }

    #[test]
    fn test_knn_search_empty_db() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.sqlite");
        let store = VectorStore::open(&db_path).unwrap();

        let query = make_query_embedding();
        let results = store.knn_search(&query, 10, 0.0).unwrap();

        assert!(results.is_empty());
    }

    #[test]
    fn test_embedding_cache() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.sqlite");
        let store = VectorStore::open(&db_path).unwrap();

        let hash = compute_content_hash("test content");
        let embedding = vec![0.5f32; EMBEDDING_DIM];

        // Cache miss
        assert!(store.get_cached_embedding(&hash).unwrap().is_none());

        // Cache the embedding
        store.cache_embedding(&hash, &embedding).unwrap();

        // Cache hit
        let cached = store.get_cached_embedding(&hash).unwrap().unwrap();
        assert_eq!(cached.len(), EMBEDDING_DIM);
        assert!((cached[0] - 0.5).abs() < 0.0001);
    }

    #[test]
    fn test_delete_by_source() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.sqlite");
        let store = VectorStore::open(&db_path).unwrap();

        // Insert chunks from different sources
        let mut chunk1 = make_test_chunk("chunk_1", "Content 1", SourceType::Topic);
        chunk1.source_name = "source_a".to_string();
        store.upsert_chunk(&chunk1).unwrap();

        let mut chunk2 = make_test_chunk("chunk_2", "Content 2", SourceType::Session);
        chunk2.source_name = "source_a".to_string();
        store.upsert_chunk(&chunk2).unwrap();

        let mut chunk3 = make_test_chunk("chunk_3", "Content 3", SourceType::Topic);
        chunk3.source_name = "source_b".to_string();
        store.upsert_chunk(&chunk3).unwrap();

        assert_eq!(store.chunk_count().unwrap(), 3);

        // Delete source_a topics (leaves the Session chunk)
        let deleted = store
            .delete_by_source(SourceType::Topic, "source_a")
            .unwrap();
        assert_eq!(deleted, 1);
        assert_eq!(store.chunk_count().unwrap(), 2);

        // Delete source_a sessions
        let deleted = store
            .delete_by_source(SourceType::Session, "source_a")
            .unwrap();
        assert_eq!(deleted, 1);
        assert_eq!(store.chunk_count().unwrap(), 1);
    }

    #[test]
    fn test_content_hash() {
        // Original basic consistency
        let hash1 = compute_content_hash("hello world");
        let hash2 = compute_content_hash("hello world");
        let hash3 = compute_content_hash("different content");

        assert_eq!(hash1, hash2);
        assert_ne!(hash1, hash3);
        assert_eq!(hash1.len(), 64); // SHA256 hex = 64 chars

        // Empty string
        let empty_hash = compute_content_hash("");
        assert_eq!(
            empty_hash,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );

        // Unicode / Emoji
        let unicode_hash = compute_content_hash("🦀 Rust");
        let unicode_hash2 = compute_content_hash("🦀 Rust");
        assert_eq!(unicode_hash, unicode_hash2);
        assert_ne!(unicode_hash, empty_hash);

        // Long string
        let long_string = "a".repeat(10000);
        let long_hash = compute_content_hash(&long_string);
        assert_eq!(long_hash.len(), 64);
    }

    #[test]
    fn test_migrate_from_json() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.sqlite");
        let store = VectorStore::open(&db_path).unwrap();

        let chunk_index = ChunkIndex {
            chunks: vec![
                make_test_chunk("chunk_1", "Content 1", SourceType::Topic),
                make_test_chunk("chunk_2", "Content 2", SourceType::Insight),
            ],
            last_rebuilt: Some(Utc::now()),
        };

        let migrated = store.migrate_from_json(&chunk_index).unwrap();
        assert_eq!(migrated, 2);
        assert_eq!(store.chunk_count().unwrap(), 2);
    }

    #[test]
    fn test_search_fts() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.sqlite");
        let store = VectorStore::open(&db_path).unwrap();

        // CHUNKS
        let c1 = make_test_chunk("c1", "The quick brown fox jumps", SourceType::Topic);
        let c2 = make_test_chunk("c2", "Lazy dog sleeps all day", SourceType::Insight);
        let c3 = make_test_chunk("c3", "Foxes are clever animals", SourceType::Topic);

        store.upsert_chunk(&c1).unwrap();
        store.upsert_chunk(&c2).unwrap();
        store.upsert_chunk(&c3).unwrap();

        // Search for "fox" (should match c1 and c3 due to stemming)
        let results = store.search_fts("fox", 10).unwrap();
        assert_eq!(results.len(), 2);
        let ids: Vec<String> = results.iter().map(|c| c.id.clone()).collect();
        assert!(ids.contains(&"c1".to_string()));
        assert!(ids.contains(&"c3".to_string()));

        // Search for "dog" (should match c2)
        let results = store.search_fts("dog", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "c2");
    }

    #[test]
    fn test_hybrid_search() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.sqlite");
        let store = VectorStore::open(&db_path).unwrap();

        // c1: "apple" (vector query will target this)
        let mut c1 = make_test_chunk("c1", "apple pie", SourceType::Topic);
        // c2: "banana" (fts query will target this)
        let mut c2 = make_test_chunk("c2", "banana split", SourceType::Topic);

        c1.embedding = vec![0.9; EMBEDDING_DIM]; // High score for query [0.9...]
        c2.embedding = vec![0.1; EMBEDDING_DIM]; // Low score

        store.upsert_chunk(&c1).unwrap();
        store.upsert_chunk(&c2).unwrap();

        let query_emb = vec![0.9; EMBEDDING_DIM];

        // Hybrid search for "banana" + vector match for "apple"
        // Should return BOTH
        let results = store.hybrid_search("banana", &query_emb, 10, 0.35).unwrap();

        assert!(results.len() >= 2);
        let ids: Vec<String> = results.iter().map(|c| c.id.clone()).collect();
        assert!(ids.contains(&"c1".to_string()));
        assert!(ids.contains(&"c2".to_string()));
    }

    #[test]
    fn test_bytes_to_f32_vec_invalid_length() {
        let bad_bytes = vec![0u8; 5];
        let result = bytes_to_f32_vec(&bad_bytes);
        assert!(result.is_err());
    }

    #[test]
    fn test_bytes_to_f32_vec_valid_roundtrip() {
        let original = vec![1.0f32; EMBEDDING_DIM];
        let bytes = f32_vec_to_bytes(&original);
        let restored = bytes_to_f32_vec(&bytes).unwrap();
        assert_eq!(original, restored);
    }

    #[test]
    fn test_upsert_chunk_rejects_wrong_dimension() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.sqlite");
        let store = VectorStore::open(&db_path).unwrap();

        let mut chunk = make_test_chunk("bad_dim", "content", SourceType::Topic);
        chunk.embedding = vec![0.1f32; 100];

        let result = store.upsert_chunk(&chunk);
        assert!(result.is_err());
    }

    #[test]
    fn test_upsert_chunk_rejects_empty_embedding() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.sqlite");
        let store = VectorStore::open(&db_path).unwrap();

        let mut chunk = make_test_chunk("empty_emb", "content", SourceType::Topic);
        chunk.embedding = vec![];

        let result = store.upsert_chunk(&chunk);
        assert!(result.is_err());
    }
}
