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

/// Pure allow-list validator. Returns the canonical logical name on success
/// so callers can `match` on a `&'static str` rather than the user-supplied
/// input. Factored out of [`resolve_allowed_path`] so the traversal +
/// allow-list guards can be unit-tested without a Tauri `AppHandle`.
pub fn validate_logical_path(logical: &str) -> Result<&'static str, String> {
    let logical = logical.trim();
    if logical.is_empty() {
        return Err("path is empty".to_string());
    }
    // Guard: no directory traversal, no absolute paths, no Windows separators.
    if logical.contains("..") || logical.starts_with('/') || logical.contains('\\') {
        return Err(format!(
            "Path '{}' is not allow-listed (must be a bare filename like 'config.toml')",
            logical
        ));
    }

    match logical {
        "config.toml" => Ok("config.toml"),
        other => Err(format!(
            "Path '{}' is not allow-listed. Allowed: config.toml",
            other
        )),
    }
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
    let canonical = validate_logical_path(logical)?;
    match canonical {
        "config.toml" => {
            let cfg_dir = app_handle
                .path()
                .app_config_dir()
                .map_err(|e| format!("Failed to resolve app config dir: {}", e))?;
            Ok(cfg_dir.join("config.toml"))
        }
        // validate_logical_path only emits known names; this is unreachable.
        other => Err(format!("Unhandled allow-listed name: {}", other)),
    }
}

/// Read the contents of an allow-listed file. Creates an empty file if the
/// target doesn't exist yet (matches normal config.toml-load behaviour).
pub fn read_allowed_file<R: Runtime>(
    app_handle: &AppHandle<R>,
    logical: &str,
) -> Result<String, String> {
    let abs = resolve_allowed_path(app_handle, logical)?;
    if !abs.exists() {
        return Ok(String::new());
    }
    fs::read_to_string(&abs).map_err(|e| format!("Failed to read {}: {}", abs.display(), e))
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

#[allow(dead_code)]
pub fn _exists_for_tests(_p: &Path) {} // marker to silence dead-code on Path import

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
}
