//! Phase 3.3 — MCP server façade.
//!
//! Exposes a curated subset of Shard's tools over the Model Context
//! Protocol's stdio transport so external coding harnesses (Claude Desktop,
//! Cursor, Codex, …) can use Shard as a memory + self-edit backend without
//! ever launching the Tauri webview.
//!
//! Entry point: [`run_stdio_server`]. The Tauri `run()` in
//! [`crate::lib`](crate) detects `--mcp` in `std::env::args` and dispatches
//! here before initialising any Tauri plugins.
//!
//! Tools exposed (curated subset of the full registry — heartbeat-only and
//! draft-gated tools are intentionally NOT exposed):
//!
//!   * `memory_search` — FTS5 search over `chunks` (topics / insights /
//!     sessions). Pure SQLite, no embedding generation needed.
//!   * `save_memory` — appends to `MEMORIES.json`.
//!   * `file_history` — wraps [`crate::file_history::summarize`].
//!   * `read_file` / `edit_file` — wraps [`crate::self_files`] with the
//!     same allow-list (`config.toml` + `personas/<slug>.md`).
//!   * `action_next` / `action_plan` — wraps [`crate::actions`].
//!
//! The server uses `dirs::data_local_dir()` directly to resolve paths
//! rather than building a fake `tauri::AppHandle` — keeps the MCP loop
//! decoupled from the Tauri runtime.

mod handlers;
mod server;

pub use handlers::{
    handle_action_next, handle_action_plan, handle_edit_file, handle_file_history,
    handle_memory_search, handle_read_file, handle_save_memory, CURATED_TOOL_NAMES,
};
pub use server::{run_stdio_server, ShardMcpServer};

use std::path::PathBuf;

/// Bundle identifier — same value the Tauri side computes via
/// `app_handle.config().identifier`. Hardcoded here so the MCP loop can
/// resolve the data dir without a runtime.
pub const SHARD_BUNDLE_ID: &str = "dev.ojasw.shard";

/// `<data_local_dir>/dev.ojasw.shard/` — root for memories.sqlite, personas,
/// MEMORIES.json, etc. Mirrors Tauri's `app_data_dir()` on macOS / Linux.
/// Created on demand.
///
/// Note: this is NOT the right base for `config.toml` on Linux — that file
/// lives under `app_config_dir()`, which `dirs::data_local_dir()` does not
/// match. Use [`shard_config_dir`] for `config.toml` so MCP-mode reads the
/// same file the GUI/agent writes.
pub fn shard_data_dir() -> Result<PathBuf, String> {
    let base =
        dirs::data_local_dir().ok_or_else(|| "Could not locate platform data dir".to_string())?;
    let dir = base.join(SHARD_BUNDLE_ID);
    if !dir.exists() {
        std::fs::create_dir_all(&dir).map_err(|e| format!("create data dir: {}", e))?;
    }
    Ok(dir)
}

/// `<config_dir>/dev.ojasw.shard/` — root for `config.toml`. Mirrors
/// Tauri's `app_config_dir()`:
///
/// | OS      | `app_config_dir()`                | `dirs::config_dir()` |
/// |---------|-----------------------------------|-----------------------|
/// | macOS   | `~/Library/Application Support/…` | `~/Library/Application Support`   |
/// | Linux   | `~/.config/…`                     | `~/.config`           |
/// | Windows | `%APPDATA%\…`                     | `%APPDATA%`           |
///
/// On macOS this happens to equal `data_local_dir()`, but on Linux the
/// two diverge (`~/.config` vs `~/.local/share`), so MCP mode must use the
/// config-dir variant to stay consistent with the Tauri-side reader.
pub fn shard_config_dir() -> Result<PathBuf, String> {
    let base =
        dirs::config_dir().ok_or_else(|| "Could not locate platform config dir".to_string())?;
    let dir = base.join(SHARD_BUNDLE_ID);
    if !dir.exists() {
        std::fs::create_dir_all(&dir).map_err(|e| format!("create config dir: {}", e))?;
    }
    Ok(dir)
}

/// SQLite path used by every storage subsystem (memories, observations,
/// actions, file_events, …).
pub fn shard_db_path() -> Result<PathBuf, String> {
    Ok(shard_data_dir()?.join("memories").join("memories.sqlite"))
}

/// Resolve an allow-listed logical path the same way
/// [`crate::self_files::resolve_allowed_path`] would, but without a Tauri
/// `AppHandle`. Kept here rather than in `self_files.rs` so the runtime
/// allow-list stays the single source of truth — we just route through
/// [`crate::self_files::classify_logical_path`].
pub fn resolve_allowed_path_no_tauri(logical: &str) -> Result<PathBuf, String> {
    use crate::self_files::{classify_logical_path, AllowedPath};
    match classify_logical_path(logical)? {
        AllowedPath::ConfigToml => {
            let base = dirs::config_dir()
                .ok_or_else(|| "Could not locate platform config dir".to_string())?;
            Ok(base.join(SHARD_BUNDLE_ID).join("config.toml"))
        }
        AllowedPath::Persona { slug } => {
            let base = dirs::data_local_dir()
                .ok_or_else(|| "Could not locate platform data dir".to_string())?;
            Ok(base
                .join(SHARD_BUNDLE_ID)
                .join("personas")
                .join(format!("{}.md", slug)))
        }
        AllowedPath::HeartbeatSpec { .. } => {
            Err("Heartbeat spec access is not permitted in MCP mode".to_string())
        }
    }
}
