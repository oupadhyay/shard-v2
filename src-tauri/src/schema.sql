-- Vector Store Schema
-- SQLite + sqlite-vec for embedding storage and KNN search

-- Main chunks table (metadata)
CREATE TABLE IF NOT EXISTS chunks (
    id TEXT PRIMARY KEY,
    source_type TEXT NOT NULL CHECK(source_type IN ('topic', 'insight')),
    source_name TEXT NOT NULL,
    heading TEXT,
    text TEXT NOT NULL,
    start_line INTEGER NOT NULL,
    end_line INTEGER NOT NULL,
    content_hash TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- vec0 virtual table for vector similarity search
-- 768 dimensions for Gemini text-embedding-004
CREATE VIRTUAL TABLE IF NOT EXISTS chunk_embeddings USING vec0(
    chunk_id TEXT PRIMARY KEY,
    embedding float[768]
);

-- Embedding cache to avoid re-embedding unchanged content
CREATE TABLE IF NOT EXISTS embedding_cache (
    content_hash TEXT PRIMARY KEY,
    embedding BLOB NOT NULL,
    created_at TEXT NOT NULL
);

-- Metadata table for store-level settings
CREATE TABLE IF NOT EXISTS metadata (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- Indexes for common queries
CREATE INDEX IF NOT EXISTS idx_chunks_source ON chunks(source_type, source_name);
CREATE INDEX IF NOT EXISTS idx_chunks_hash ON chunks(content_hash);

-- FTS5 table for keyword search
-- We use 'porter' stemmer for better recall
-- chunk_id is UNINDEXED but stored to link back to main table
CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(
    heading,
    text,
    chunk_id UNINDEXED,
    tokenize='porter'
);

-- Triggers to sync content automatically from 'chunks' to 'chunks_fts'

CREATE TRIGGER IF NOT EXISTS chunks_ai AFTER INSERT ON chunks BEGIN
  INSERT INTO chunks_fts(chunk_id, heading, text) VALUES (new.id, new.heading, new.text);
END;

CREATE TRIGGER IF NOT EXISTS chunks_ad AFTER DELETE ON chunks BEGIN
  DELETE FROM chunks_fts WHERE chunk_id = old.id;
END;

CREATE TRIGGER IF NOT EXISTS chunks_au AFTER UPDATE ON chunks BEGIN
  UPDATE chunks_fts SET heading = new.heading, text = new.text WHERE chunk_id = old.id;
END;
