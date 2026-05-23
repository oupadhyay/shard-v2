//! Phase 3.1 — Pre-compaction hook that snapshots the agent's open sketches
//! (action / frontier planner state) into the daily memory log before the
//! conversation is summarized.
//!
//! Without this hook, a long-running multi-file refactor that triggers
//! compaction mid-flight would lose the per-turn frontier injection on the
//! next turn (because process.rs queries `pending_sketch_summary` from
//! SQLite, but the agent's *narrative* context about *why* the plan was
//! created would have been compacted). Writing a short "open sketches"
//! blurb to the daily log keeps the recipe discoverable by RAG.

use std::sync::Arc;

use crate::agent::hooks::LifecycleHooks;
use crate::vector_store::VectorStore;

pub struct ActionsHook<R: tauri::Runtime> {
    app_handle: tauri::AppHandle<R>,
}

impl<R: tauri::Runtime> ActionsHook<R> {
    pub fn new(app_handle: tauri::AppHandle<R>) -> Arc<Self> {
        Arc::new(Self { app_handle })
    }

    fn open_store(&self) -> Option<VectorStore> {
        crate::memories::get_vector_store(&self.app_handle).ok()
    }
}

impl<R: tauri::Runtime> LifecycleHooks for ActionsHook<R> {
    fn on_pre_compact(&self, session_id: &str, _history_tokens: usize) {
        let Some(store) = self.open_store() else {
            return;
        };
        let Some(snapshot) = crate::actions::pending_sketch_summary_text(&store) else {
            return; // no open sketches → nothing to preserve
        };

        let header = format!(
            "\n## Open action sketches at compaction ({}, session={})\n",
            chrono::Utc::now().format("%Y-%m-%d %H:%M"),
            session_id
        );
        let content = format!("{}{}\n", header, snapshot);

        if let Err(e) = crate::memories::append_to_daily_log(&self.app_handle, &content) {
            log::warn!(
                "[hooks::actions] Failed to write open-sketch snapshot to daily log: {}",
                e
            );
        } else {
            log::info!(
                "[hooks::actions] Wrote open-sketch snapshot ({} chars) to daily log",
                snapshot.len()
            );
        }
    }
}
