//! Phase 1.1 — Lifecycle hooks.
//!
//! Single integration point for cross-cutting concerns that previously had to
//! be sprinkled across `agent/process.rs`, `agent/tools/mod.rs`, and
//! `compaction.rs`. Hooks fire at four boundaries:
//!
//! | Boundary           | Method                | Fired from                  |
//! |--------------------|-----------------------|-----------------------------|
//! | session start      | `on_session_start`    | (reserved for Phase 2)      |
//! | pre-tool-use       | `on_pre_tool_use`     | `Agent::execute_tool`       |
//! | post-tool-use      | `on_post_tool_use`    | `Agent::execute_tool`       |
//! | pre-compaction     | `on_pre_compact`      | `Agent::process_message`    |
//! | session end        | `on_session_end`      | (reserved for Phase 2)      |
//!
//! All methods have default no-op implementations so a hook can opt into just
//! the boundaries it cares about. Panics inside a hook are caught via
//! `catch_unwind` at the dispatcher (see `dispatch_*` helpers) so one
//! misbehaving hook can never abort an agent turn.

pub mod actions_hook;
pub mod file_history_hook;

use serde_json::Value;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;

/// Lightweight view of a tool call passed to `on_pre_tool_use`.
pub struct ToolInvocation<'a> {
    pub name: &'a str,
    pub args: &'a Value,
    pub call_id: Option<&'a str>,
}

/// Lightweight view of a tool result passed to `on_post_tool_use`.
pub struct ToolOutcome<'a> {
    pub name: &'a str,
    pub args: &'a Value,
    pub call_id: Option<&'a str>,
    pub result: &'a str,
    pub is_error: bool,
}

/// Decision the dispatcher makes after running pre-tool hooks.
#[derive(Debug, Clone)]
pub enum HookOutcome {
    /// Proceed with normal tool execution.
    Continue,
    /// Replace the tool result with the embedded string and skip execution
    /// (used for tool-call caching / file-history short-circuits in Phase 2).
    Replace(String),
    /// Refuse the tool call entirely and propagate the message back to the
    /// model as a tool error.
    Abort(String),
}

/// User-implementable trait. All methods are no-ops by default; implement
/// just the ones you need.
pub trait LifecycleHooks: Send + Sync {
    fn on_session_start(&self, _session_id: &str) {}
    fn on_pre_tool_use(&self, _call: &ToolInvocation<'_>) -> HookOutcome {
        HookOutcome::Continue
    }
    fn on_post_tool_use(&self, _outcome: &ToolOutcome<'_>) {}
    fn on_pre_compact(&self, _session_id: &str, _history_tokens: usize) {}
    fn on_session_end(&self, _session_id: &str) {}
}

/// Concrete registry stored on `Agent`. Wraps an immutable `Vec` of `Arc<dyn
/// LifecycleHooks>` so dispatch is lock-free on the hot path and registration
/// only happens at construction time.
#[derive(Default, Clone)]
pub struct HookRegistry {
    hooks: Vec<Arc<dyn LifecycleHooks>>,
}

impl HookRegistry {
    pub fn new() -> Self {
        Self { hooks: Vec::new() }
    }

    pub fn push(&mut self, hook: Arc<dyn LifecycleHooks>) {
        self.hooks.push(hook);
    }

    pub fn len(&self) -> usize {
        self.hooks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.hooks.is_empty()
    }

    pub fn dispatch_session_start(&self, session_id: &str) {
        for h in &self.hooks {
            let h = Arc::clone(h);
            let _ = catch_unwind(AssertUnwindSafe(|| h.on_session_start(session_id)));
        }
    }

    /// First-non-Continue wins. Hooks run in registration order; the first
    /// `Replace` or `Abort` short-circuits subsequent hooks. Panics inside a
    /// hook are downgraded to `Continue` so they cannot kill the turn.
    pub fn dispatch_pre_tool(&self, call: &ToolInvocation<'_>) -> HookOutcome {
        for h in &self.hooks {
            let h = Arc::clone(h);
            let result = catch_unwind(AssertUnwindSafe(|| h.on_pre_tool_use(call)));
            match result {
                Ok(HookOutcome::Continue) => continue,
                Ok(outcome) => return outcome,
                Err(_) => {
                    log::warn!("[hooks] pre_tool_use panicked; downgrading to Continue");
                    continue;
                }
            }
        }
        HookOutcome::Continue
    }

    pub fn dispatch_post_tool(&self, outcome: &ToolOutcome<'_>) {
        for h in &self.hooks {
            let h = Arc::clone(h);
            let _ = catch_unwind(AssertUnwindSafe(|| h.on_post_tool_use(outcome)));
        }
    }

    pub fn dispatch_pre_compact(&self, session_id: &str, history_tokens: usize) {
        for h in &self.hooks {
            let h = Arc::clone(h);
            let _ = catch_unwind(AssertUnwindSafe(|| {
                h.on_pre_compact(session_id, history_tokens)
            }));
        }
    }

    pub fn dispatch_session_end(&self, session_id: &str) {
        for h in &self.hooks {
            let h = Arc::clone(h);
            let _ = catch_unwind(AssertUnwindSafe(|| h.on_session_end(session_id)));
        }
    }
}
