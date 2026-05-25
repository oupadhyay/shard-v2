-- Vector Store Schema
-- SQLite + sqlite-vec for embedding storage and KNN search

-- Main chunks table (metadata)
CREATE TABLE IF NOT EXISTS chunks (
    id TEXT PRIMARY KEY,
    source_type TEXT NOT NULL CHECK(source_type IN ('topic', 'insight', 'session')),
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
-- 768 dimensions for Gemini Embedding 2 (output_dimensionality=768)
CREATE VIRTUAL TABLE IF NOT EXISTS chunk_embeddings USING vec0(
    chunk_id TEXT PRIMARY KEY,
    embedding float[768] distance_metric=cosine
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

-- Unified Session Model (Conversations)

CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    summary TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    active_skills TEXT DEFAULT '[]'
);

CREATE TABLE IF NOT EXISTS messages (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    created_at TEXT NOT NULL,
    FOREIGN KEY(session_id) REFERENCES sessions(id) ON DELETE CASCADE
);

-- ============================================================================
-- Honcho-style Peer-Centric Observations
-- ============================================================================
-- Observations are leveled facts about entities (typically the user).
-- Inspired by Honcho's Collection(observer, observed) + Document(level, source_ids) model.
--
-- Levels form a DAG:
--   explicit  — Direct extraction from a message ("User said they live in SF")
--   deductive — Logical implication from 1+ explicit ("User likely commutes in Bay Area")
--   inductive — Pattern across 2+ observations ("User prefers West Coast cities")
--   contradiction — Flagged conflict between observations
--
-- No migration needed: all statements use IF NOT EXISTS.

CREATE TABLE IF NOT EXISTS observations (
    id TEXT PRIMARY KEY,
    observer TEXT NOT NULL DEFAULT 'shard',
    observed TEXT NOT NULL DEFAULT 'user',
    content TEXT NOT NULL,
    level TEXT NOT NULL CHECK(level IN ('explicit', 'deductive', 'inductive', 'contradiction')),
    source_ids TEXT NOT NULL DEFAULT '[]',
    times_derived INTEGER NOT NULL DEFAULT 0,
    session_name TEXT,
    content_hash TEXT NOT NULL,
    created_at TEXT NOT NULL,
    deleted_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_obs_observed ON observations(observed, level, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_obs_observer ON observations(observer, observed);
CREATE INDEX IF NOT EXISTS idx_obs_session ON observations(session_name);
CREATE INDEX IF NOT EXISTS idx_obs_hash ON observations(content_hash);

-- sqlite-vec virtual table for observation embedding search (768-dim Gemini Embedding 2)
CREATE VIRTUAL TABLE IF NOT EXISTS observation_embeddings USING vec0(
    observation_id TEXT PRIMARY KEY,
    embedding float[768] distance_metric=cosine
);

-- FTS5 for keyword search over observations
CREATE VIRTUAL TABLE IF NOT EXISTS observations_fts USING fts5(
    content,
    observation_id UNINDEXED,
    tokenize='porter'
);

-- Auto-sync triggers for observations → observations_fts
CREATE TRIGGER IF NOT EXISTS obs_fts_ai AFTER INSERT ON observations BEGIN
  INSERT INTO observations_fts(observation_id, content) VALUES (new.id, new.content);
END;

CREATE TRIGGER IF NOT EXISTS obs_fts_ad AFTER DELETE ON observations BEGIN
  DELETE FROM observations_fts WHERE observation_id = old.id;
END;

-- Phase 1.3: scope this trigger to UPDATEs of `content` only. Decay-related
-- writes (`decay_score`, `last_accessed`, `deleted_at`) happen for every
-- observation on every sweep and don't change the searchable text — letting
-- this trigger reindex FTS5 anyway turned a 10k-row sweep into a ~11 s job.
CREATE TRIGGER IF NOT EXISTS obs_fts_au AFTER UPDATE OF content ON observations BEGIN
  UPDATE observations_fts SET content = new.content WHERE observation_id = old.id;
END;

-- Peer card: curated biographical summary per observer×observed pair.
-- Mirrors Honcho's Collection.internal_metadata.peer_card.
CREATE TABLE IF NOT EXISTS peer_cards (
    observer TEXT NOT NULL DEFAULT 'shard',
    observed TEXT NOT NULL DEFAULT 'user',
    facts TEXT NOT NULL DEFAULT '[]',
    updated_at TEXT NOT NULL,
    PRIMARY KEY (observer, observed)
);

-- ============================================================================
-- Phase 1.2 — SHA-256 dedup window
-- ============================================================================
-- Short-lived content-hash registry used to skip re-storing the same
-- observation or tool result when the agent revisits a fact within a small
-- time window (default 5 min). Hot-path reads are served by an in-memory
-- HashMap in dedup.rs; this table is the durable mirror.

CREATE TABLE IF NOT EXISTS dedup_window (
    content_hash TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('observation','tool_result')),
    first_seen TEXT NOT NULL,
    last_seen TEXT NOT NULL,
    hit_count INTEGER NOT NULL DEFAULT 1,
    PRIMARY KEY (content_hash, kind)
);

CREATE INDEX IF NOT EXISTS idx_dedup_seen ON dedup_window(last_seen);

-- ============================================================================
-- Phase 2.1 — File-centric memory
-- ============================================================================
-- Per-file event log used by the `file_history` tool to surface "you've
-- edited this 4 times in 7d, last edit was followed by a test failure" before
-- the agent does another `edit_file`. Lightweight: write-once rows, no FTS,
-- no embeddings. Errors that show up within a short window after an edit are
-- back-filled by the post-tool lifecycle hook.

CREATE TABLE IF NOT EXISTS file_events (
    id TEXT PRIMARY KEY,
    logical_path TEXT NOT NULL,
    abs_path TEXT NOT NULL,
    event_kind TEXT NOT NULL CHECK(event_kind IN ('read','edit','revert','snapshot')),
    session_id TEXT,
    before_hash TEXT,
    after_hash TEXT,
    -- Phase 2.3: optional snapshot of the pre-edit content so `rollback_self_edit`
    -- can restore the file without a separate git store. Capped to ~64KB at
    -- insertion (see file_history::SNAPSHOT_SIZE_CAP); large files just lose
    -- per-event rollback (snapshot stays NULL).
    before_content TEXT,
    unified_diff TEXT,
    followed_by_error TEXT,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_file_events_path
    ON file_events(logical_path, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_file_events_recent
    ON file_events(created_at DESC);

-- ============================================================================
-- Phase 3.1 — Action / Frontier planner
-- ============================================================================
-- Persistent task graph for multi-step self-edits. `parent_id` lets actions
-- be grouped into "sketches" (a parent action with N children). `deps` is a
-- JSON array of action ids that must reach status 'done' before this action
-- becomes ready. `frontier()` returns the highest-priority ready action.
-- Kept separate from `proactive_queue` to avoid mixing draft-approval and
-- task-planning semantics.

CREATE TABLE IF NOT EXISTS actions (
    id TEXT PRIMARY KEY,
    parent_id TEXT,
    title TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK(status IN ('pending','active','done','blocked','cancelled')),
    priority INTEGER NOT NULL DEFAULT 0,
    deps TEXT NOT NULL DEFAULT '[]',
    payload TEXT,
    session_id TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    block_reason TEXT,
    outcome TEXT
);

CREATE INDEX IF NOT EXISTS idx_actions_status
    ON actions(status, priority DESC, created_at);
CREATE INDEX IF NOT EXISTS idx_actions_parent ON actions(parent_id);
CREATE INDEX IF NOT EXISTS idx_actions_session ON actions(session_id);
