//! Phase 1.2 — SHA-256 dedup window.
//!
//! Short-lived content-hash registry that suppresses re-storing the same
//! observation or tool result when the agent revisits a fact within a small
//! window (default 5 min).
//!
//! Two layers:
//!  * **In-memory `HashMap`** (`HOT_CACHE`) keyed by `(hash, kind)` for sub-µs
//!    hot-path reads. Entries older than `MAX_WINDOW_SECS` are lazily evicted
//!    on access.
//!  * **`dedup_window` SQLite table** for durability across restarts and for
//!    surfacing high-hit signatures (loop detection — Phase 1.2 instrumentation).
//!
//! The cache is intentionally global (not per-`VectorStore`) because the
//! actual `Connection` lives behind `OnceLock` in `memories::get_vector_store`
//! and the agent only ever has one DB open at a time in production.

use rusqlite::{params, OptionalExtension};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::vector_store::VectorStore;

/// Maximum window we will keep entries around for. Long-lived enough that
/// repeated reads of `config.toml` within a turn collapse, short enough that
/// genuinely re-derived facts after a real edit get re-recorded.
pub const DEFAULT_WINDOW_SECS: u64 = 300;

/// Hard cap on the in-memory cache. Larger than any realistic window — we
/// rely on lazy eviction.
const HOT_CACHE_CAP: usize = 4096;

/// Threshold above which we emit a warning log. Indicates the agent is
/// likely in a tool-call loop reading/writing the same content.
const LOOP_WARN_THRESHOLD: u32 = 20;

/// Kind of payload being dedup'd. Different kinds never collide even with
/// identical hashes — an observation and a tool result might be byte-equal
/// but mean very different things.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DedupKind {
    Observation,
    ToolResult,
}

impl DedupKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Observation => "observation",
            Self::ToolResult => "tool_result",
        }
    }
}

#[derive(Clone, Copy)]
struct HotEntry {
    /// Monotonic clock instant the entry was inserted.
    inserted: Instant,
    hit_count: u32,
}

static HOT_CACHE: Mutex<Option<HashMap<(String, DedupKind), HotEntry>>> = Mutex::new(None);

fn with_cache<R>(f: impl FnOnce(&mut HashMap<(String, DedupKind), HotEntry>) -> R) -> R {
    let mut guard = HOT_CACHE.lock().expect("dedup cache poisoned");
    let map = guard.get_or_insert_with(HashMap::new);
    f(map)
}

/// Returns `true` if `(hash, kind)` was already seen within `window`.
/// Increments the hit counter (both in-memory and durable) when it's a dup.
///
/// On a cache miss the entry is inserted with `hit_count = 1` and `false`
/// is returned, signalling the caller should proceed with the write.
pub fn is_duplicate(store: &VectorStore, hash: &str, kind: DedupKind, window: Duration) -> bool {
    let now = Instant::now();
    let key = (hash.to_string(), kind);

    let (dup, hit) = with_cache(|cache| {
        // Lazy eviction of stale entries (bounded work — at most cache size).
        if cache.len() > HOT_CACHE_CAP {
            cache.retain(|_, e| now.duration_since(e.inserted) < window);
        }
        match cache.get_mut(&key) {
            Some(entry) if now.duration_since(entry.inserted) < window => {
                entry.hit_count = entry.hit_count.saturating_add(1);
                (true, entry.hit_count)
            }
            _ => {
                cache.insert(
                    key.clone(),
                    HotEntry {
                        inserted: now,
                        hit_count: 1,
                    },
                );
                (false, 1)
            }
        }
    });

    // Mirror to durable table. The hot path (dup hit) intentionally does
    // NOT touch SQLite — the in-memory cache is authoritative and the
    // durable table is purely a restart-survival aid. Only the initial
    // insert and the periodic loop-warning path issue SQL.
    if dup {
        if hit >= LOOP_WARN_THRESHOLD && hit.is_power_of_two() {
            log::warn!(
                "[dedup] loop suspected: kind={} hash={}… hits={}",
                kind.as_str(),
                &hash[..hash.len().min(12)],
                hit,
            );
            // Only on the warning boundary do we sync the hit_count durably
            // so post-mortem inspection of the table reflects the loop.
            let nowstr = chrono::Utc::now().to_rfc3339();
            let _ = store.conn.execute(
                "UPDATE dedup_window SET hit_count = ?1, last_seen = ?2 \
                 WHERE content_hash = ?3 AND kind = ?4",
                params![hit as i64, nowstr, hash, kind.as_str()],
            );
        }
    } else {
        let nowstr = chrono::Utc::now().to_rfc3339();
        let _ = store.conn.execute(
            "INSERT OR REPLACE INTO dedup_window \
             (content_hash, kind, first_seen, last_seen, hit_count) \
             VALUES (?1, ?2, ?3, ?3, 1)",
            params![hash, kind.as_str(), nowstr],
        );
    }

    dup
}

/// In-memory hit count for `(hash, kind)`. Reflects the authoritative
/// counter (the durable `dedup_window.hit_count` only lags during
/// non-loop traffic). Returns 0 if the entry has expired or never existed.
pub fn peek_hit_count_memory(hash: &str, kind: DedupKind) -> u32 {
    with_cache(|cache| {
        cache
            .get(&(hash.to_string(), kind))
            .map(|e| e.hit_count)
            .unwrap_or(0)
    })
}

/// Lookup-only variant used in tests and observability — never mutates the
/// hit counter or inserts.
pub fn peek_hit_count(store: &VectorStore, hash: &str, kind: DedupKind) -> u32 {
    store
        .conn
        .query_row(
            "SELECT hit_count FROM dedup_window WHERE content_hash = ?1 AND kind = ?2",
            params![hash, kind.as_str()],
            |row| row.get::<_, i64>(0).map(|v| v as u32),
        )
        .optional()
        .unwrap_or(None)
        .unwrap_or(0)
}

/// Clear the in-memory hot cache. Intended for tests that need a fresh
/// state without restarting the process.
pub fn reset_hot_cache_for_testing() {
    let mut guard = HOT_CACHE.lock().expect("dedup cache poisoned");
    *guard = Some(HashMap::new());
}

/// Drop durable rows older than `older_than`. Called by background sweep.
pub fn sweep_durable(store: &VectorStore, older_than: chrono::Duration) -> Result<usize, String> {
    let cutoff = (chrono::Utc::now() - older_than).to_rfc3339();
    store
        .conn
        .execute(
            "DELETE FROM dedup_window WHERE last_seen < ?1",
            params![cutoff],
        )
        .map(|n| n as usize)
        .map_err(|e| format!("dedup sweep failed: {}", e))
}
