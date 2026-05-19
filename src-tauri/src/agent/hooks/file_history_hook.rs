//! Phase 2.1 — Post-tool hook that attributes tool errors back to a recent
//! `file_events.edit` row. This is what turns a raw event log into the
//! "⚠️ Caution: last edit caused a test failure" prompt the `file_history`
//! tool surfaces.
//!
//! The hook only fires on `is_error == true` for tools that plausibly
//! exercise the recently-edited file. We keep the list narrow to avoid
//! false attribution (e.g. an unrelated `get_weather` failure should not
//! mark a config edit as having "caused" anything).

use std::sync::Arc;

use crate::agent::hooks::{LifecycleHooks, ToolOutcome};
use crate::self_files::validate_logical_path;
use crate::vector_store::VectorStore;

/// Tools whose errors are considered diagnostic feedback for prior edits.
/// Keep this conservative — adding `web_search` here would mis-attribute
/// network blips to file edits.
const SELF_EDIT_FEEDBACK_TOOLS: &[&str] = &[
    "run_python",
    "read_file",
    "edit_file",
    "file_history",
];

pub struct FileHistoryHook<R: tauri::Runtime> {
    app_handle: tauri::AppHandle<R>,
}

impl<R: tauri::Runtime> FileHistoryHook<R> {
    pub fn new(app_handle: tauri::AppHandle<R>) -> Arc<Self> {
        Arc::new(Self { app_handle })
    }

    fn open_store(&self) -> Option<VectorStore> {
        crate::memories::get_vector_store(&self.app_handle).ok()
    }
}

impl<R: tauri::Runtime> LifecycleHooks for FileHistoryHook<R> {
    fn on_post_tool_use(&self, outcome: &ToolOutcome<'_>) {
        if !outcome.is_error {
            return;
        }
        if !SELF_EDIT_FEEDBACK_TOOLS.contains(&outcome.name) {
            return;
        }

        let Some(store) = self.open_store() else {
            return;
        };

        // Attribute the error to whichever allow-listed paths have a recent
        // edit. For most tool calls there's only ever 0 or 1 candidate, so
        // walking the (small) allow-list is fine.
        for candidate in allow_listed_paths() {
            if validate_logical_path(candidate).is_err() {
                continue;
            }
            let _ = crate::file_history::attribute_error_to_recent_edits(
                &store,
                candidate,
                outcome.result,
            );
        }
    }
}

/// Snapshot of the allow-list visible to the hook. We don't query the
/// runtime allow-list directly because [`crate::self_files`] keeps it as a
/// match arm rather than data — duplicating the names here is cheap and
/// keeps this hook decoupled.
fn allow_listed_paths() -> &'static [&'static str] {
    &["config.toml"]
}
