//! Shared infrastructure for agent unit tests.
//!
//! Two cross-cutting concerns make agent tests tricky:
//!
//!   1. `tauri::test::mock_app()` resolves `app_data_dir` against `$HOME` via
//!      `dirs`. If two tests run in parallel and each sets `$HOME` to its own
//!      tempdir, they race. We serialize all agent tests with [`AGENT_TEST_LOCK`]
//!      and rebuild the global `crate::endpoints` overrides for each test.
//!
//!   2. The agent emits Tauri events; tests that need to assert on them install
//!      listeners up-front. [`Captured`] and [`register_listeners`] mirror what
//!      the eval harness does in `examples/eval.rs` so we don't reinvent the
//!      wheel.
//!
//! Typical test shape:
//!
//! ```ignore
//! let _g = AGENT_TEST_LOCK.lock().unwrap();
//! let env = TestEnv::new();   // tempdir, HOME, mock server, endpoints override
//! let agent = Agent::new(env.handle.clone());
//! // ...mount wiremock expectations on env.server, then call agent.process_message(...)
//! ```

#![cfg(test)]
// Helpers are progressively used as Phases 1-5 add tests; silence the
// per-helper dead-code warnings until then.
#![allow(dead_code)]

use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use tauri::{AppHandle, Listener};
use wiremock::MockServer;

use crate::endpoints::{self, Endpoints};

/// The single canonical process-wide lock guarding `$HOME` (and the global
/// endpoint overrides). `$HOME` is shared mutable process state, so **every**
/// test that redirects it — agent, mcp, heartbeat, persona, crystals — must
/// serialize on *this* lock. Using per-module locks only serializes within a
/// group and lets tests in different groups clobber each other's `$HOME` (and
/// therefore each other's on-disk SQLite DB), which is the historical source
/// of flaky "no such table" failures.
pub fn home_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let m = LOCK.get_or_init(|| Mutex::new(()));
    // Recover from poisoning so a panicking test doesn't break every other test.
    match m.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Process-wide lock around `$HOME` mutation + endpoint overrides. Every
/// agent test must hold this for its full duration. Alias for [`home_lock`].
pub fn agent_test_lock() -> MutexGuard<'static, ()> {
    home_lock()
}

/// RAII guard that redirects `$HOME` to a fresh tempdir for the duration of a
/// test, so `app_data_dir()` / `dirs::data_local_dir()` resolve inside an
/// isolated sandbox. **Must** be created while holding [`home_lock`].
pub struct HomeJail {
    _td: tempfile::TempDir,
    prev: Option<std::ffi::OsString>,
}

impl HomeJail {
    pub fn new() -> Self {
        let td = tempfile::Builder::new()
            .prefix("shard-test-")
            .tempdir()
            .expect("tempdir");
        let prev = std::env::var_os("HOME");
        std::env::set_var("HOME", td.path());
        Self { _td: td, prev }
    }
}

impl Default for HomeJail {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for HomeJail {
    fn drop(&mut self) {
        match self.prev.take() {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }
}

/// One-stop fixture: tempdir-rooted `$HOME`, a fresh `wiremock::MockServer`
/// pointed at by `crate::endpoints`, and a built `tauri::AppHandle` whose
/// `app_data_dir()` resolves inside the tempdir.
///
/// Drop semantics: clearing endpoint overrides happens in `Drop` so even
/// panicking tests leave the global state clean for the next test.
pub struct TestEnv {
    pub handle: AppHandle<tauri::test::MockRuntime>,
    pub server: MockServer,
    pub _tempdir: tempfile::TempDir,
    /// Saved `$HOME` so we can restore on drop. Cell so we can take it.
    prev_home: Option<std::ffi::OsString>,
}

impl TestEnv {
    pub async fn new() -> Self {
        let tempdir = tempfile::Builder::new()
            .prefix("shard-agent-test-")
            .tempdir()
            .expect("tempdir");

        // Redirect $HOME so app_data_dir() lands inside the sandbox.
        let prev_home = std::env::var_os("HOME");
        std::env::set_var("HOME", tempdir.path());

        // Spin up an HTTP mock server and point all endpoints at it.
        let server = MockServer::start().await;
        let base = server.uri();
        endpoints::set_overrides(Endpoints {
            gemini_interactions: format!("{}/v1beta/interactions", base),
            gemini_classify: format!("{}/v1beta/models/classifier:generateContent", base),
            gemini_files_base: format!("{}/v1beta/files", base),
            gemini_files_upload: format!("{}/upload/v1beta/files", base),
            gemini_embedding: format!("{}/v1beta/models/embedder:embedContent", base),
            openrouter_chat: format!("{}/openrouter/chat/completions", base),
            groq_chat: format!("{}/groq/chat/completions", base),
        });

        // Build a Tauri mock app inside the sandbox.
        let app = tauri::test::mock_app();
        let handle = app.handle().clone();

        Self {
            handle,
            server,
            _tempdir: tempdir,
            prev_home,
        }
    }
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        endpoints::clear_overrides();
        if let Some(prev) = self.prev_home.take() {
            std::env::set_var("HOME", prev);
        } else {
            std::env::remove_var("HOME");
        }
    }
}

// ============================================================================
// Event capture
// ============================================================================

#[derive(Debug, Default, Clone)]
pub struct CapturedEvents {
    pub responses: Vec<String>,
    pub reasoning: Vec<String>,
    pub tool_calls: Vec<String>,
    pub tool_results: Vec<String>,
    pub errors: Vec<String>,
    pub retries: Vec<String>,
    pub retries_exhausted: Vec<String>,
    pub user_messages: Vec<String>,
    pub fallbacks: Vec<String>,
    pub compactions: Vec<String>,
}

pub type Captured = Arc<Mutex<CapturedEvents>>;

pub fn captured() -> Captured {
    Arc::new(Mutex::new(CapturedEvents::default()))
}

/// Install listeners for every agent-emitted event we test against.
pub fn register_listeners<R: tauri::Runtime>(handle: &AppHandle<R>, cap: Captured) {
    macro_rules! listen_str {
        ($event:literal, $field:ident) => {{
            let cap = cap.clone();
            handle.listen($event, move |e| {
                cap.lock()
                    .unwrap()
                    .$field
                    .push(strip_quotes(e.payload()).to_string());
            });
        }};
    }
    listen_str!("agent-response-chunk", responses);
    listen_str!("agent-reasoning-chunk", reasoning);
    listen_str!("agent-tool-call", tool_calls);
    listen_str!("agent-tool-result", tool_results);
    listen_str!("agent-error", errors);
    listen_str!("agent-retry", retries);
    listen_str!("agent-retry-exhausted", retries_exhausted);
    listen_str!("user-message", user_messages);
    listen_str!("agent-fallback", fallbacks);
    listen_str!("agent-compaction", compactions);
}

/// Strip the JSON-quoting Tauri applies to string payloads. Returns the inner
/// string without enclosing quotes; pass-through for non-quoted payloads.
fn strip_quotes(s: &str) -> &str {
    let trimmed = s.trim();
    if trimmed.len() >= 2 && trimmed.starts_with('"') && trimmed.ends_with('"') {
        &trimmed[1..trimmed.len() - 1]
    } else {
        trimmed
    }
}

// ============================================================================
// SSE response builders
// ============================================================================

/// Build a Gemini Interactions API SSE body from raw events. Each entry is a
/// JSON value that becomes one `data: {...}\n\n` block.
pub fn gemini_sse(events: &[serde_json::Value]) -> String {
    let mut out = String::new();
    for ev in events {
        out.push_str("data: ");
        out.push_str(&ev.to_string());
        out.push_str("\n\n");
    }
    out
}

/// Build an OpenAI/OpenRouter-style SSE chat completion body, terminating
/// with `data: [DONE]`.
pub fn openrouter_sse(events: &[serde_json::Value]) -> String {
    let mut out = String::new();
    for ev in events {
        out.push_str("data: ");
        out.push_str(&ev.to_string());
        out.push_str("\n\n");
    }
    out.push_str("data: [DONE]\n\n");
    out
}

/// Convenience: a single OpenRouter delta with text content.
pub fn or_delta_text(text: &str) -> serde_json::Value {
    serde_json::json!({
        "choices": [{"index": 0, "delta": {"content": text}}]
    })
}

/// Convenience: a single OpenRouter delta with reasoning content.
pub fn or_delta_reasoning(text: &str) -> serde_json::Value {
    serde_json::json!({
        "choices": [{"index": 0, "delta": {"reasoning": text}}]
    })
}

// ============================================================================
// Smoke tests for the helpers themselves
// ============================================================================

#[cfg(test)]
mod helper_smoke {
    use super::*;

    #[tokio::test]
    async fn test_env_builds_and_overrides_endpoints() {
        let _g = agent_test_lock();
        let env = TestEnv::new().await;
        // The override should now point at the wiremock server.
        let interactions = endpoints::gemini_interactions();
        assert!(interactions.starts_with(&env.server.uri()));
        assert!(interactions.ends_with("/v1beta/interactions"));
        // app_data_dir should resolve under the temp HOME (sandboxed).
        let dir = tauri::Manager::path(&env.handle).app_data_dir().unwrap();
        assert!(dir.starts_with(env._tempdir.path()));
        drop(env);
        // Drop must restore defaults.
        assert_eq!(
            endpoints::gemini_interactions(),
            "https://generativelanguage.googleapis.com/v1beta/interactions"
        );
    }

    #[test]
    fn sse_builders_format_correctly() {
        let body = openrouter_sse(&[or_delta_text("hi")]);
        assert!(body.contains("\"content\":\"hi\""));
        assert!(body.ends_with("data: [DONE]\n\n"));

        let g = gemini_sse(&[serde_json::json!({"candidates":[]})]);
        assert!(g.starts_with("data: "));
        assert!(g.ends_with("\n\n"));
        assert!(!g.contains("[DONE]"));
    }
}
