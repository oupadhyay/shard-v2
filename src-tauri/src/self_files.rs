//! Self-editing file helper.
//!
//! Backs the agent's generic `read_file` / `edit_file` tools. Maintains an
//! allow-list mapping logical paths (the LLM-visible name) to absolute paths
//! on disk, performs old_str → new_str substring edits, and produces a
//! structured outcome (before / after / unified diff) for both the tool result
//! and a `file-edited` Tauri event consumed by the frontend diff viewer.
//!
//! Adding a new editable file = add an entry to [`ALLOWED_FILES`]. Heartbeat
//! spec files and (later) implementation source files plug in here, so we
//! don't grow a new tool per surface.
//!
//! Sensitive-content guards live alongside each entry — `config.toml` refuses
//! to keep any `*_api_key` field (those live in the OS keychain only).
//!
//! The agent loop calls this from `agent/tools/mod.rs::execute_tool_uncached`.

use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager, Runtime};

#[derive(Debug, Serialize, Clone)]
pub struct EditOutcome {
    pub path: String,        // logical (LLM-facing) path
    pub abs_path: String,    // absolute resolved path
    pub before: String,
    pub after: String,
    pub unified_diff: String, // human/diff-viewer friendly
    pub replacements: usize,  // number of substitutions actually made
}

/// Logical-path classification returned by [`validate_logical_path`]. Lets
/// callers branch on a typed enum instead of stringly-matching on the
/// canonical path — important now that the allow-list spans multiple
/// directories (`personas/<slug>.md` lives outside the app config dir).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AllowedPath {
    ConfigToml,
    /// `personas/<slug>.md` — `slug` is the validated bare slug, without the
    /// `.md` suffix.
    Persona { slug: String },
    /// `heartbeats/<name>.toml` — `name` is the validated bare slug, without the
    /// `.toml` suffix.
    HeartbeatSpec { name: String },
}

/// Pure allow-list validator. Returns the canonical logical name on success
/// so callers can `match` on a `&'static str` rather than the user-supplied
/// input. Factored out of [`resolve_allowed_path`] so the traversal +
/// allow-list guards can be unit-tested without a Tauri `AppHandle`.
///
/// Backwards-compatible shim around [`classify_logical_path`] — returns the
/// canonical name string. Prefer `classify_logical_path` in new code.
pub fn validate_logical_path(logical: &str) -> Result<&'static str, String> {
    match classify_logical_path(logical)? {
        AllowedPath::ConfigToml => Ok("config.toml"),
        AllowedPath::Persona { .. } => Ok("persona"),
        AllowedPath::HeartbeatSpec { .. } => Ok("heartbeat"),
    }
}

/// Validate a slug for `heartbeats/<name>.toml`. Must match `[a-z][a-z0-9-]{1,40}`.
/// Refuses leading hyphens/digits, uppercase, underscores, or anything else
/// that would surprise the existing `heartbeats/<name>.toml` reader.
pub fn validate_heartbeat_slug(slug: &str) -> Result<(), String> {
    let len = slug.len();
    if !(2..=41).contains(&len) {
        return Err(format!(
            "heartbeat slug '{}' must be 2-41 chars (got {})",
            slug, len
        ));
    }
    let mut chars = slug.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_lowercase() {
        return Err(format!(
            "heartbeat slug '{}' must start with a lowercase letter [a-z]",
            slug
        ));
    }
    for c in chars {
        let ok = c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-';
        if !ok {
            return Err(format!(
                "heartbeat slug '{}' may only contain [a-z0-9-] (offending char: {:?})",
                slug, c
            ));
        }
    }
    Ok(())
}

/// Validate a slug for `personas/<slug>.md`. Must match `[a-z][a-z0-9-]{1,40}`.
/// Refuses leading hyphens/digits, uppercase, underscores, or anything else
/// that would surprise the existing `personas/<name>.md` reader.
pub fn validate_persona_slug(slug: &str) -> Result<(), String> {
    let len = slug.len();
    if !(2..=41).contains(&len) {
        return Err(format!(
            "persona slug '{}' must be 2-41 chars (got {})",
            slug, len
        ));
    }
    let mut chars = slug.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_lowercase() {
        return Err(format!(
            "persona slug '{}' must start with a lowercase letter [a-z]",
            slug
        ));
    }
    for c in chars {
        let ok = c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-';
        if !ok {
            return Err(format!(
                "persona slug '{}' may only contain [a-z0-9-] (offending char: {:?})",
                slug, c
            ));
        }
    }
    Ok(())
}

/// Typed variant of [`validate_logical_path`]. Returns a structured
/// [`AllowedPath`] so call-sites can carry the slug forward without a
/// second parse.
pub fn classify_logical_path(logical: &str) -> Result<AllowedPath, String> {
    let logical = logical.trim();
    if logical.is_empty() {
        return Err("path is empty".to_string());
    }
    // Guard: no `..`, no absolute paths, no Windows separators.
    if logical.contains("..") || logical.starts_with('/') || logical.contains('\\') {
        return Err(format!(
            "Path '{}' is not allow-listed (must be e.g. 'config.toml' or 'personas/<slug>.md')",
            logical
        ));
    }

    if logical == "config.toml" {
        return Ok(AllowedPath::ConfigToml);
    }
    if let Some(rest) = logical.strip_prefix("personas/") {
        let slug = rest.strip_suffix(".md").ok_or_else(|| {
            format!(
                "persona path '{}' must end in '.md' (e.g. personas/news-analyst.md)",
                logical
            )
        })?;
        // The slash split + `..` ban above means `slug` is already a bare
        // filename, but enforce the regex contract explicitly.
        validate_persona_slug(slug)?;
        return Ok(AllowedPath::Persona {
            slug: slug.to_string(),
        });
    }
    if let Some(rest) = logical.strip_prefix("heartbeats/") {
        let name = rest.strip_suffix(".toml").ok_or_else(|| {
            format!(
                "heartbeat path '{}' must end in '.toml' (e.g. heartbeats/news-analyst.toml)",
                logical
            )
        })?;
        validate_heartbeat_slug(name)?;
        return Ok(AllowedPath::HeartbeatSpec {
            name: name.to_string(),
        });
    }

    Err(format!(
        "Path '{}' is not allow-listed. Allowed: config.toml, personas/<slug>.md, heartbeats/<name>.toml",
        logical
    ))
}

/// Resolve a logical allow-listed path to an absolute path under the app's
/// config dir. Currently the only entry is `config.toml`.
///
/// We resolve via the app's config dir rather than CWD so unit tests and
/// production both behave identically.
pub fn resolve_allowed_path<R: Runtime>(
    app_handle: &AppHandle<R>,
    logical: &str,
) -> Result<PathBuf, String> {
    match classify_logical_path(logical)? {
        AllowedPath::ConfigToml => {
            let cfg_dir = app_handle
                .path()
                .app_config_dir()
                .map_err(|e| format!("Failed to resolve app config dir: {}", e))?;
            Ok(cfg_dir.join("config.toml"))
        }
        AllowedPath::Persona { slug } => {
            // Personas live under `dirs::data_local_dir()/dev.ojasw.shard/personas/<slug>.md`
            // — same root as `crate::personas::get_personas_dir`, but we
            // intentionally re-derive here so this module stays usable in
            // unit tests that don't load the full personas subsystem.
            let dir = crate::personas::get_personas_dir()?;
            Ok(dir.join(format!("{}.md", slug)))
        }
        AllowedPath::HeartbeatSpec { name } => {
            let dir = crate::heartbeat::get_heartbeats_dir(app_handle)?;
            Ok(dir.join(format!("{}.toml", name)))
        }
    }
}

/// Read the contents of an allow-listed file. Creates an empty file if the
/// target doesn't exist yet (matches normal config.toml-load behaviour).
///
/// Phase 2.1: records a `read` event in `file_events` so the `file_history`
/// tool can reason about access cadence. Failures recording the event are
/// non-fatal — the read result is still returned to the agent.
pub fn read_allowed_file<R: Runtime>(
    app_handle: &AppHandle<R>,
    logical: &str,
) -> Result<String, String> {
    let abs = resolve_allowed_path(app_handle, logical)?;
    let contents = if !abs.exists() {
        String::new()
    } else {
        fs::read_to_string(&abs).map_err(|e| format!("Failed to read {}: {}", abs.display(), e))?
    };

    if let Ok(store) = crate::memories::get_vector_store(app_handle) {
        let session_id = current_session_id(app_handle);
        let _ = crate::file_history::record_read(
            &store,
            crate::file_history::RecordRead {
                logical_path: logical,
                abs_path: &abs.display().to_string(),
                content: &contents,
                session_id: session_id.as_deref(),
            },
        );
    }

    Ok(contents)
}

/// Best-effort current session id for file_event attribution.
/// Returns `None` if the agent state isn't available (e.g. during eval).
fn current_session_id<R: Runtime>(app_handle: &AppHandle<R>) -> Option<String> {
    use tauri::Manager;
    let state = app_handle.try_state::<crate::AppState>()?;
    // We can't block on a tokio Mutex from sync code; clone the agent and
    // try a non-blocking lock. If contended, fall back to None — file_history
    // session attribution is best-effort.
    let agent = state.agent.clone();
    let sid = agent.session_id.try_lock().ok().map(|g| g.clone());
    sid
}

/// Apply an old_str -> new_str edit to an allow-listed file. Returns a
/// structured [`EditOutcome`] containing before/after content and a unified
/// diff suitable for both display and a diff-viewer event.
pub fn edit_allowed_file<R: Runtime>(
    app_handle: &AppHandle<R>,
    logical: &str,
    old_str: &str,
    new_str: &str,
    replace_all: bool,
) -> Result<EditOutcome, String> {
    let abs = resolve_allowed_path(app_handle, logical)?;

    // Per-file sensitive-content guards.
    if logical == "config.toml" {
        guard_no_api_key(old_str)?;
        guard_no_api_key(new_str)?;
    }

    let before = if abs.exists() {
        fs::read_to_string(&abs).map_err(|e| format!("Failed to read {}: {}", abs.display(), e))?
    } else {
        String::new()
    };

    let (after, replacements) = apply_edit(&before, old_str, new_str, replace_all)?;

    if let Some(parent) = abs.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create parent dir: {}", e))?;
        }
    }
    fs::write(&abs, &after).map_err(|e| format!("Failed to write {}: {}", abs.display(), e))?;

    let unified_diff = unified_diff(&before, &after, logical);

    // Phase 2.1: log to file_events so file_history / post-tool hooks can
    // reason about cadence and error attribution. Best-effort — failures
    // never block the edit from returning success.
    if let Ok(store) = crate::memories::get_vector_store(app_handle) {
        let session_id = current_session_id(app_handle);
        let abs_str = abs.display().to_string();
        let _ = crate::file_history::record_edit(
            &store,
            crate::file_history::RecordEdit {
                logical_path: logical,
                abs_path: &abs_str,
                before: &before,
                after: &after,
                unified_diff: &unified_diff,
                session_id: session_id.as_deref(),
            },
        );
    }

    Ok(EditOutcome {
        path: logical.to_string(),
        abs_path: abs.display().to_string(),
        before,
        after,
        unified_diff,
        replacements,
    })
}

/// Pure path-based variant of [`edit_allowed_file`] that does not require
/// a Tauri `AppHandle`. Used by the MCP server façade in `crate::mcp`
/// where there is no live Tauri runtime. The caller is responsible for
/// resolving `abs` via [`crate::mcp::resolve_allowed_path_no_tauri`] (or
/// equivalent) and for routing the returned [`EditOutcome`] into any
/// downstream logging (`file_events`, diff-viewer events, …).
///
/// Applies the same per-file content guards (`config.toml` refuses any
/// `*_api_key` string) and the same `apply_edit` substring semantics.
pub fn edit_at_abs_path(
    abs: &Path,
    logical: &str,
    old_str: &str,
    new_str: &str,
    replace_all: bool,
) -> Result<EditOutcome, String> {
    if logical == "config.toml" {
        guard_no_api_key(old_str)?;
        guard_no_api_key(new_str)?;
    }

    let before = if abs.exists() {
        fs::read_to_string(abs).map_err(|e| format!("Failed to read {}: {}", abs.display(), e))?
    } else {
        String::new()
    };

    let (after, replacements) = apply_edit(&before, old_str, new_str, replace_all)?;

    if let Some(parent) = abs.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create parent dir: {}", e))?;
        }
    }
    fs::write(abs, &after).map_err(|e| format!("Failed to write {}: {}", abs.display(), e))?;

    let unified_diff = unified_diff(&before, &after, logical);

    Ok(EditOutcome {
        path: logical.to_string(),
        abs_path: abs.display().to_string(),
        before,
        after,
        unified_diff,
        replacements,
    })
}

/// Pure substring edit, factored out for unit testing.
pub fn apply_edit(
    haystack: &str,
    old_str: &str,
    new_str: &str,
    replace_all: bool,
) -> Result<(String, usize), String> {
    if old_str.is_empty() {
        if !haystack.is_empty() {
            return Err(
                "old_str is empty but file is non-empty; cannot infer where to insert".to_string(),
            );
        }
        return Ok((new_str.to_string(), 1));
    }

    let occurrences = haystack.matches(old_str).count();
    if occurrences == 0 {
        return Err(format!(
            "old_str not found in file. Tip: call read_file first and copy the exact text (including whitespace)."
        ));
    }
    if !replace_all && occurrences > 1 {
        return Err(format!(
            "old_str matches {} times; pass replace_all=true or extend old_str to make it unique",
            occurrences
        ));
    }

    let (out, n) = if replace_all {
        (haystack.replace(old_str, new_str), occurrences)
    } else {
        (haystack.replacen(old_str, new_str, 1), 1)
    };
    Ok((out, n))
}

/// Defensive: refuse anything that looks like an API-key TOML assignment.
fn guard_no_api_key(s: &str) -> Result<(), String> {
    let lower = s.to_ascii_lowercase();
    const FORBIDDEN: &[&str] = &[
        "api_key",
        "gemini_api_key",
        "openrouter_api_key",
        "brave_api_key",
        "groq_api_key",
    ];
    for needle in FORBIDDEN {
        if lower.contains(needle) {
            return Err(format!(
                "Refusing edit: text contains '{}' — API keys live in the OS keychain and are stripped from config.toml on save. Ask the user to set keys via Settings.",
                needle
            ));
        }
    }
    Ok(())
}

/// Tiny line-based unified-diff (no external deps). Good enough for the
/// tool-call accordion and for the frontend diff viewer to fall back on if
/// it just wants plaintext.
fn unified_diff(before: &str, after: &str, label: &str) -> String {
    let a: Vec<&str> = before.split_inclusive('\n').collect();
    let b: Vec<&str> = after.split_inclusive('\n').collect();
    let mut out = String::new();
    out.push_str(&format!("--- a/{}\n", label));
    out.push_str(&format!("+++ b/{}\n", label));

    // Naive LCS-free diff: emit removed lines for the longest common prefix
    // mismatch, then added lines. For small config edits this is fine.
    let prefix = common_prefix_len(&a, &b);
    let suffix = common_suffix_len(&a[prefix..], &b[prefix..]);
    let a_changed = &a[prefix..a.len() - suffix];
    let b_changed = &b[prefix..b.len() - suffix];

    if a_changed.is_empty() && b_changed.is_empty() {
        out.push_str("(no textual change)\n");
        return out;
    }

    let ctx_before = prefix.saturating_sub(3);
    let ctx_after_end = std::cmp::min(a.len() - suffix + 3, a.len());
    out.push_str(&format!(
        "@@ -{},{} +{},{} @@\n",
        ctx_before + 1,
        ctx_after_end - ctx_before,
        ctx_before + 1,
        (ctx_after_end - ctx_before) + b_changed.len() - a_changed.len()
    ));
    for line in &a[ctx_before..prefix] {
        out.push(' ');
        out.push_str(line);
        if !line.ends_with('\n') {
            out.push('\n');
        }
    }
    for line in a_changed {
        out.push('-');
        out.push_str(line);
        if !line.ends_with('\n') {
            out.push('\n');
        }
    }
    for line in b_changed {
        out.push('+');
        out.push_str(line);
        if !line.ends_with('\n') {
            out.push('\n');
        }
    }
    for line in &a[a.len() - suffix..ctx_after_end] {
        out.push(' ');
        out.push_str(line);
        if !line.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

fn common_prefix_len<'a, T: PartialEq>(a: &[T], b: &[T]) -> usize {
    let mut i = 0;
    let n = std::cmp::min(a.len(), b.len());
    while i < n && a[i] == b[i] {
        i += 1;
    }
    i
}

fn common_suffix_len<T: PartialEq>(a: &[T], b: &[T]) -> usize {
    let mut i = 0;
    let n = std::cmp::min(a.len(), b.len());
    while i < n && a[a.len() - 1 - i] == b[b.len() - 1 - i] {
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_edit_replaces_unique() {
        let (out, n) = apply_edit("hello world", "world", "Rust", false).unwrap();
        assert_eq!(out, "hello Rust");
        assert_eq!(n, 1);
    }

    #[test]
    fn apply_edit_rejects_ambiguous() {
        let err = apply_edit("foo foo", "foo", "bar", false).unwrap_err();
        assert!(err.contains("matches 2 times"));
    }

    #[test]
    fn apply_edit_replace_all() {
        let (out, n) = apply_edit("foo foo", "foo", "bar", true).unwrap();
        assert_eq!(out, "bar bar");
        assert_eq!(n, 2);
    }

    #[test]
    fn apply_edit_missing_old_str() {
        let err = apply_edit("hello", "world", "bye", false).unwrap_err();
        assert!(err.contains("not found"));
    }

    #[test]
    fn apply_edit_empty_to_empty_file_creates_content() {
        let (out, n) = apply_edit("", "", "fresh", false).unwrap();
        assert_eq!(out, "fresh");
        assert_eq!(n, 1);
    }

    #[test]
    fn apply_edit_empty_old_on_non_empty_file_errors() {
        let err = apply_edit("nonempty", "", "x", false).unwrap_err();
        assert!(err.contains("non-empty"));
    }

    #[test]
    fn guard_blocks_api_key() {
        assert!(guard_no_api_key("gemini_api_key = \"abc\"").is_err());
        assert!(guard_no_api_key("openrouter_api_key=\"\"").is_err());
        assert!(guard_no_api_key("selected_model = \"gpt-4\"").is_ok());
    }

    #[test]
    fn unified_diff_shows_removed_and_added() {
        let diff = unified_diff(
            "alpha\nbeta\ngamma\n",
            "alpha\nBETA\ngamma\n",
            "config.toml",
        );
        assert!(diff.contains("--- a/config.toml"));
        assert!(diff.contains("+++ b/config.toml"));
        assert!(diff.contains("-beta\n"));
        assert!(diff.contains("+BETA\n"));
    }

    #[test]
    fn unified_diff_no_change() {
        let diff = unified_diff("same\n", "same\n", "config.toml");
        assert!(diff.contains("(no textual change)"));
    }

    // ── Allow-list guard (pure) ────────────────────────────────────────

    #[test]
    fn validate_accepts_known_path() {
        assert_eq!(validate_logical_path("config.toml").unwrap(), "config.toml");
        // Whitespace is trimmed.
        assert_eq!(
            validate_logical_path("  config.toml  ").unwrap(),
            "config.toml"
        );
    }

    #[test]
    fn validate_rejects_empty_path() {
        assert!(validate_logical_path("").unwrap_err().contains("empty"));
        assert!(validate_logical_path("   ").unwrap_err().contains("empty"));
    }

    #[test]
    fn validate_rejects_traversal_and_absolute_paths() {
        for bad in &[
            "../foo",
            "../../etc/passwd",
            "/etc/passwd",
            "/abs/path",
            "bar/../baz",
            "config.toml/../secret",
            r"C:\Windows\System32",
        ] {
            let err = validate_logical_path(bad).unwrap_err();
            assert!(
                err.contains("not allow-listed"),
                "expected traversal/abs reject for '{}', got: {}",
                bad,
                err
            );
        }
    }

    #[test]
    fn validate_rejects_unknown_allow_listed_name() {
        let err = validate_logical_path("nonexistent.toml").unwrap_err();
        assert!(err.contains("not allow-listed"));
        assert!(err.contains("config.toml"), "should hint allowed list");
    }

    // ── Phase 3.2: personas/<slug>.md allow-list arm ───────────────────

    #[test]
    fn classify_accepts_persona_path() {
        let out = classify_logical_path("personas/news-analyst.md").unwrap();
        assert_eq!(
            out,
            AllowedPath::Persona {
                slug: "news-analyst".to_string()
            }
        );
    }

    #[test]
    fn classify_rejects_persona_missing_md_suffix() {
        let err = classify_logical_path("personas/news-analyst").unwrap_err();
        assert!(err.contains(".md"));
    }

    #[test]
    fn classify_rejects_persona_bad_slug() {
        // Must start with a lowercase letter.
        assert!(classify_logical_path("personas/1analyst.md").is_err());
        assert!(classify_logical_path("personas/-bad.md").is_err());
        assert!(classify_logical_path("personas/UPPER.md").is_err());
        assert!(classify_logical_path("personas/snake_case.md").is_err());
        assert!(classify_logical_path("personas/a.md").is_err()); // too short
        assert!(classify_logical_path("personas/abc def.md").is_err()); // space
    }

    #[test]
    fn classify_rejects_persona_traversal() {
        // The `..` ban catches escape attempts before slug validation runs.
        assert!(classify_logical_path("personas/../config.toml").is_err());
        assert!(classify_logical_path("personas/./hidden.md").is_err());
    }

    #[test]
    fn validate_persona_slug_accepts_valid() {
        validate_persona_slug("news").unwrap();
        validate_persona_slug("news-analyst").unwrap();
        validate_persona_slug("a1").unwrap();
        validate_persona_slug("multi-word-slug-1").unwrap();
    }

    #[test]
    fn validate_persona_slug_length_bounds() {
        assert!(validate_persona_slug("").is_err());
        assert!(validate_persona_slug("a").is_err());
        // 41 chars: 'a' + 40 hyphens-style chars — exact length OK.
        let max = format!("a{}", "1".repeat(40));
        assert_eq!(max.len(), 41);
        validate_persona_slug(&max).unwrap();
        // 42 chars: too long.
        let over = format!("a{}", "1".repeat(41));
        assert!(validate_persona_slug(&over).is_err());
    }

    // Heartbeat classification tests
    #[test]
    fn classify_accepts_heartbeat_path() {
        let out = classify_logical_path("heartbeats/daily-review.toml").unwrap();
        assert_eq!(
            out,
            AllowedPath::HeartbeatSpec {
                name: "daily-review".to_string()
            }
        );
    }

    #[test]
    fn classify_rejects_heartbeat_bad_slug() {
        assert!(classify_logical_path("heartbeats/1review.toml").is_err());
        assert!(classify_logical_path("heartbeats/UPPER.toml").is_err());
        assert!(classify_logical_path("heartbeats/news-analyst").is_err()); // missing suffix
        assert!(classify_logical_path("heartbeats/../news.toml").is_err()); // traversal
        assert!(classify_logical_path("heartbeats/sub/news.toml").is_err()); // nested folder
    }
}
