//! Phase 2.1 — File-centric memory.
//!
//! Append-only event log of every read/edit/revert/snapshot the agent
//! performs on an allow-listed file (see [`crate::self_files`]). Drives the
//! `file_history` tool, which the agent calls before non-trivial `edit_file`
//! turns so it can see prior diffs, edit cadence, and whether previous edits
//! were followed by errors.
//!
//! Errors that surface within [`ERROR_ATTRIBUTION_WINDOW`] of an edit are
//! retroactively attached to the most recent matching `file_events` row by
//! [`attribute_error_to_recent_edits`], called from the post-tool lifecycle
//! hook in `agent/hooks/file_history_hook.rs`.

use crate::vector_store::{compute_content_hash, VectorStore};
use chrono::{DateTime, Utc};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

/// How long after an `edit` event do we still consider a tool error to be
/// a consequence of that edit. Calibrated to match a single agent turn.
pub const ERROR_ATTRIBUTION_WINDOW: chrono::Duration = chrono::Duration::seconds(60);

/// Maximum `before_content` snapshot length stored per edit event. Edits
/// to files larger than this still record an event (with hashes and diff)
/// but cannot be rolled back via `rollback_event` — the agent must instead
/// re-derive the desired state and call `edit_file` again. 64 KiB covers
/// realistic config and persona files comfortably.
pub const SNAPSHOT_SIZE_CAP: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileEventKind {
    Read,
    Edit,
    Revert,
    Snapshot,
}

impl FileEventKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Edit => "edit",
            Self::Revert => "revert",
            Self::Snapshot => "snapshot",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "read" => Some(Self::Read),
            "edit" => Some(Self::Edit),
            "revert" => Some(Self::Revert),
            "snapshot" => Some(Self::Snapshot),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEvent {
    pub id: String,
    pub logical_path: String,
    pub abs_path: String,
    pub event_kind: FileEventKind,
    pub session_id: Option<String>,
    pub before_hash: Option<String>,
    pub after_hash: Option<String>,
    pub unified_diff: Option<String>,
    pub followed_by_error: Option<String>,
    pub created_at: String,
}

/// Caller-friendly record for the post-tool hook. We accept lengths rather
/// than full content for read events so we don't bloat the DB.
pub struct RecordRead<'a> {
    pub logical_path: &'a str,
    pub abs_path: &'a str,
    pub content: &'a str,
    pub session_id: Option<&'a str>,
}

pub fn record_read(store: &VectorStore, ev: RecordRead<'_>) -> Result<String, String> {
    let id = uuid::Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let hash = compute_content_hash(ev.content);
    store
        .conn
        .execute(
            "INSERT INTO file_events \
             (id, logical_path, abs_path, event_kind, session_id, \
              before_hash, after_hash, unified_diff, followed_by_error, created_at) \
             VALUES (?1, ?2, ?3, 'read', ?4, NULL, ?5, NULL, NULL, ?6)",
            params![id, ev.logical_path, ev.abs_path, ev.session_id, hash, now],
        )
        .map_err(|e| format!("record_read failed: {}", e))?;
    Ok(id)
}

pub struct RecordEdit<'a> {
    pub logical_path: &'a str,
    pub abs_path: &'a str,
    pub before: &'a str,
    pub after: &'a str,
    pub unified_diff: &'a str,
    pub session_id: Option<&'a str>,
}

pub fn record_edit(store: &VectorStore, ev: RecordEdit<'_>) -> Result<String, String> {
    let id = uuid::Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let before_hash = compute_content_hash(ev.before);
    let after_hash = compute_content_hash(ev.after);
    // Phase 2.3: snapshot the pre-edit content so rollback can restore it
    // exactly. Only when the file is small enough that the snapshot stays
    // light on the DB — large files trade rollback for storage.
    let before_snapshot: Option<&str> = if ev.before.len() <= SNAPSHOT_SIZE_CAP {
        Some(ev.before)
    } else {
        None
    };
    store
        .conn
        .execute(
            "INSERT INTO file_events \
             (id, logical_path, abs_path, event_kind, session_id, \
              before_hash, after_hash, before_content, unified_diff, \
              followed_by_error, created_at) \
             VALUES (?1, ?2, ?3, 'edit', ?4, ?5, ?6, ?7, ?8, NULL, ?9)",
            params![
                id,
                ev.logical_path,
                ev.abs_path,
                ev.session_id,
                before_hash,
                after_hash,
                before_snapshot,
                ev.unified_diff,
                now
            ],
        )
        .map_err(|e| format!("record_edit failed: {}", e))?;
    Ok(id)
}

/// Phase 2.3 — rollback the most recent restorable edit for `logical_path`,
/// or a specific `event_id` if supplied. Writes the snapshot back to
/// `abs_path` and records a `revert` event so the history stays auditable.
///
/// Returns `(reverted_event_id, after_content_len)` on success. If no
/// matching event has a stored snapshot the call returns an error rather
/// than silently doing nothing.
pub fn rollback_event(
    store: &VectorStore,
    logical_path: &str,
    event_id: Option<&str>,
) -> Result<(String, usize), String> {
    let (id, abs_path, before): (String, String, String) = match event_id {
        Some(eid) => store
            .conn
            .query_row(
                "SELECT id, abs_path, before_content FROM file_events \
                 WHERE id = ?1 AND logical_path = ?2 \
                       AND event_kind = 'edit' AND before_content IS NOT NULL",
                params![eid, logical_path],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .map_err(|e| format!("no rollback target for event {}: {}", eid, e))?,
        None => store
            .conn
            .query_row(
                "SELECT id, abs_path, before_content FROM file_events \
                 WHERE logical_path = ?1 \
                       AND event_kind = 'edit' AND before_content IS NOT NULL \
                 ORDER BY created_at DESC LIMIT 1",
                params![logical_path],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .map_err(|e| format!("no restorable edit for {}: {}", logical_path, e))?,
    };

    std::fs::write(&abs_path, &before)
        .map_err(|e| format!("rollback write to {} failed: {}", abs_path, e))?;

    // Record the revert as its own event so file_history surfaces it. We do
    // NOT call record_edit here because there's no semantic diff to attach;
    // the rollback's "diff" is the inverse of the original edit.
    let revert_id = uuid::Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let new_hash = compute_content_hash(&before);
    store
        .conn
        .execute(
            "INSERT INTO file_events \
             (id, logical_path, abs_path, event_kind, session_id, \
              before_hash, after_hash, before_content, unified_diff, \
              followed_by_error, created_at) \
             VALUES (?1, ?2, ?3, 'revert', NULL, NULL, ?4, NULL, ?5, NULL, ?6)",
            params![
                revert_id,
                logical_path,
                abs_path,
                new_hash,
                format!("(rolled back to event {})", id),
                now
            ],
        )
        .map_err(|e| format!("rollback event insert failed: {}", e))?;

    Ok((id, before.len()))
}

/// Update the most recent `edit` event for `logical_path` if it happened
/// within [`ERROR_ATTRIBUTION_WINDOW`]. Returns the number of rows updated
/// (0 or 1). Called by the post-tool hook when a tool reports an error.
pub fn attribute_error_to_recent_edits(
    store: &VectorStore,
    logical_path: &str,
    error_message: &str,
) -> Result<usize, String> {
    let cutoff = (Utc::now() - ERROR_ATTRIBUTION_WINDOW).to_rfc3339();
    let recent: Option<(String, String)> = store
        .conn
        .query_row(
            "SELECT id, created_at FROM file_events \
             WHERE logical_path = ? AND event_kind = 'edit' \
                   AND followed_by_error IS NULL \
                   AND created_at >= ? \
             ORDER BY created_at DESC LIMIT 1",
            params![logical_path, cutoff],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(|e| format!("attribute_error lookup failed: {}", e))?;
    let Some((id, _ts)) = recent else {
        return Ok(0);
    };
    let n = store
        .conn
        .execute(
            "UPDATE file_events SET followed_by_error = ? WHERE id = ?",
            params![error_message, id],
        )
        .map_err(|e| format!("attribute_error update failed: {}", e))?;
    Ok(n)
}

pub fn get_events(
    store: &VectorStore,
    logical_path: &str,
    limit: usize,
) -> Result<Vec<FileEvent>, String> {
    let mut stmt = store
        .conn
        .prepare(
            "SELECT id, logical_path, abs_path, event_kind, session_id, \
                    before_hash, after_hash, unified_diff, followed_by_error, created_at \
             FROM file_events WHERE logical_path = ? \
             ORDER BY created_at DESC LIMIT ?",
        )
        .map_err(|e| e.to_string())?;
    let rows: Vec<FileEvent> = stmt
        .query_map(params![logical_path, limit as i64], |row| {
            Ok(FileEvent {
                id: row.get(0)?,
                logical_path: row.get(1)?,
                abs_path: row.get(2)?,
                event_kind: FileEventKind::parse(&row.get::<_, String>(3)?).unwrap_or(FileEventKind::Read),
                session_id: row.get(4).ok(),
                before_hash: row.get(5).ok(),
                after_hash: row.get(6).ok(),
                unified_diff: row.get(7).ok(),
                followed_by_error: row.get(8).ok(),
                created_at: row.get(9)?,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

/// LLM-facing summary: a compact prose blurb + the latest N raw events.
/// Designed for direct interpolation into a `file_history` tool result.
pub fn summarize(store: &VectorStore, logical_path: &str, limit: usize) -> Result<String, String> {
    let events = get_events(store, logical_path, limit)?;
    if events.is_empty() {
        return Ok(format!(
            "No prior reads, edits, reverts, or snapshots recorded for `{}`.",
            logical_path
        ));
    }

    let edits = events
        .iter()
        .filter(|e| e.event_kind == FileEventKind::Edit)
        .count();
    let reads = events
        .iter()
        .filter(|e| e.event_kind == FileEventKind::Read)
        .count();
    let errored_edits = events
        .iter()
        .filter(|e| e.event_kind == FileEventKind::Edit && e.followed_by_error.is_some())
        .count();
    let last_edit_age = events
        .iter()
        .find(|e| e.event_kind == FileEventKind::Edit)
        .and_then(|e| {
            DateTime::parse_from_rfc3339(&e.created_at)
                .ok()
                .map(|t| (Utc::now() - t.with_timezone(&Utc)).num_hours())
        });

    let mut out = String::new();
    out.push_str(&format!("# History for `{}`\n\n", logical_path));
    out.push_str(&format!(
        "- **Summary:** {} edit{}, {} read{}, {} edit{} followed by a tool error.",
        edits,
        if edits == 1 { "" } else { "s" },
        reads,
        if reads == 1 { "" } else { "s" },
        errored_edits,
        if errored_edits == 1 { "" } else { "s" },
    ));
    if let Some(hours) = last_edit_age {
        if hours <= 0 {
            out.push_str(" Last edit was minutes ago.\n");
        } else if hours == 1 {
            out.push_str(" Last edit was 1 hour ago.\n");
        } else {
            out.push_str(&format!(" Last edit was {} hours ago.\n", hours));
        }
    } else {
        out.push('\n');
    }
    if errored_edits > 0 {
        out.push_str(
            "- **⚠️ Caution:** prior edits to this file have triggered tool errors. Read the file and the failing test output before editing again.\n",
        );
    }
    out.push('\n');

    out.push_str(&format!(
        "## Recent events (most recent {} shown)\n\n",
        events.len()
    ));
    for ev in &events {
        let when = &ev.created_at;
        let session = ev
            .session_id
            .as_deref()
            .unwrap_or("(no session)");
        out.push_str(&format!(
            "### {} • {} • session {}\n",
            ev.event_kind.as_str(),
            when,
            session
        ));
        if let Some(diff) = ev.unified_diff.as_ref() {
            // Cap diff length per event so the tool result stays under typical
            // model context budgets.
            let diff_capped = if diff.len() > 1200 {
                format!("{}\n... (diff truncated)\n", &diff[..1200])
            } else {
                diff.clone()
            };
            out.push_str(&format!("```diff\n{}\n```\n", diff_capped));
        }
        if let Some(err) = ev.followed_by_error.as_ref() {
            let err_capped = if err.len() > 400 {
                format!("{}…", &err[..400])
            } else {
                err.clone()
            };
            out.push_str(&format!("**Followed by error:** {}\n", err_capped));
        }
        out.push('\n');
    }
    Ok(out)
}
