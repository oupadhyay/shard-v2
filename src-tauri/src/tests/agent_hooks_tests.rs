//! Phase 1.1 — Lifecycle hooks dispatcher tests.
//!
//! These tests exercise [`HookRegistry`] directly (no Tauri runtime required)
//! to prove ordering, short-circuit behavior, and panic isolation. Wiring into
//! `Agent::execute_tool` and `process_message` is covered by the broader
//! agent test suite once we add fixture coverage for them.

use crate::agent::hooks::{HookOutcome, HookRegistry, LifecycleHooks, ToolInvocation, ToolOutcome};
use serde_json::json;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

#[derive(Default)]
struct CountingHook {
    pre_tool: AtomicUsize,
    post_tool: AtomicUsize,
    session_start: AtomicUsize,
    session_end: AtomicUsize,
    pre_compact: AtomicUsize,
}

impl LifecycleHooks for CountingHook {
    fn on_session_start(&self, _session_id: &str) {
        self.session_start.fetch_add(1, Ordering::SeqCst);
    }
    fn on_pre_tool_use(&self, _call: &ToolInvocation<'_>) -> HookOutcome {
        self.pre_tool.fetch_add(1, Ordering::SeqCst);
        HookOutcome::Continue
    }
    fn on_post_tool_use(&self, _outcome: &ToolOutcome<'_>) {
        self.post_tool.fetch_add(1, Ordering::SeqCst);
    }
    fn on_pre_compact(&self, _session_id: &str, _history_tokens: usize) {
        self.pre_compact.fetch_add(1, Ordering::SeqCst);
    }
    fn on_session_end(&self, _session_id: &str) {
        self.session_end.fetch_add(1, Ordering::SeqCst);
    }
}

struct OrderedHook {
    id: usize,
    record: Arc<std::sync::Mutex<Vec<usize>>>,
}

impl LifecycleHooks for OrderedHook {
    fn on_pre_tool_use(&self, _call: &ToolInvocation<'_>) -> HookOutcome {
        self.record.lock().unwrap().push(self.id);
        HookOutcome::Continue
    }
    fn on_post_tool_use(&self, _outcome: &ToolOutcome<'_>) {}
}

struct PanicHook;
impl LifecycleHooks for PanicHook {
    fn on_pre_tool_use(&self, _call: &ToolInvocation<'_>) -> HookOutcome {
        panic!("simulated hook panic");
    }
    fn on_post_tool_use(&self, _outcome: &ToolOutcome<'_>) {
        panic!("simulated post-hook panic");
    }
}

struct ReplaceHook(String);
impl LifecycleHooks for ReplaceHook {
    fn on_pre_tool_use(&self, _call: &ToolInvocation<'_>) -> HookOutcome {
        HookOutcome::Replace(self.0.clone())
    }
}

struct AbortHook(String);
impl LifecycleHooks for AbortHook {
    fn on_pre_tool_use(&self, _call: &ToolInvocation<'_>) -> HookOutcome {
        HookOutcome::Abort(self.0.clone())
    }
}

fn make_invocation<'a>(name: &'a str, args: &'a serde_json::Value) -> ToolInvocation<'a> {
    ToolInvocation {
        name,
        args,
        call_id: None,
    }
}

fn make_outcome<'a>(name: &'a str, args: &'a serde_json::Value, result: &'a str) -> ToolOutcome<'a> {
    ToolOutcome {
        name,
        args,
        call_id: None,
        result,
        is_error: false,
    }
}

#[test]
fn hook_fires_on_each_lifecycle_event() {
    let counter = Arc::new(CountingHook::default());
    let mut registry = HookRegistry::new();
    registry.push(counter.clone());

    let args = json!({"q": "hi"});
    let inv = make_invocation("web_search", &args);
    let out = make_outcome("web_search", &args, "ok");

    registry.dispatch_session_start("s1");
    let _ = registry.dispatch_pre_tool(&inv);
    registry.dispatch_post_tool(&out);
    registry.dispatch_pre_compact("s1", 100_000);
    registry.dispatch_session_end("s1");

    assert_eq!(counter.session_start.load(Ordering::SeqCst), 1);
    assert_eq!(counter.pre_tool.load(Ordering::SeqCst), 1);
    assert_eq!(counter.post_tool.load(Ordering::SeqCst), 1);
    assert_eq!(counter.pre_compact.load(Ordering::SeqCst), 1);
    assert_eq!(counter.session_end.load(Ordering::SeqCst), 1);
}

#[test]
fn multiple_hooks_run_in_registration_order() {
    let record = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut registry = HookRegistry::new();
    for id in [1, 2, 3] {
        registry.push(Arc::new(OrderedHook {
            id,
            record: record.clone(),
        }));
    }
    let args = json!({});
    let inv = make_invocation("noop", &args);
    let _ = registry.dispatch_pre_tool(&inv);
    assert_eq!(*record.lock().unwrap(), vec![1, 2, 3]);
}

#[test]
fn pre_tool_use_replace_short_circuits() {
    let counter = Arc::new(CountingHook::default());
    let mut registry = HookRegistry::new();
    registry.push(Arc::new(ReplaceHook("cached".to_string())));
    registry.push(counter.clone()); // Should not be hit

    let args = json!({});
    let inv = make_invocation("web_search", &args);
    match registry.dispatch_pre_tool(&inv) {
        HookOutcome::Replace(s) => assert_eq!(s, "cached"),
        other => panic!("expected Replace, got {:?}", other),
    }
    // The second hook never fired because the first returned non-Continue.
    assert_eq!(counter.pre_tool.load(Ordering::SeqCst), 0);
}

#[test]
fn pre_tool_use_abort_propagates_message() {
    let mut registry = HookRegistry::new();
    registry.push(Arc::new(AbortHook("dangerous".to_string())));
    let args = json!({});
    let inv = make_invocation("rm_rf", &args);
    match registry.dispatch_pre_tool(&inv) {
        HookOutcome::Abort(msg) => assert_eq!(msg, "dangerous"),
        other => panic!("expected Abort, got {:?}", other),
    }
}

#[test]
fn hook_failure_in_one_does_not_kill_others() {
    let counter = Arc::new(CountingHook::default());
    let mut registry = HookRegistry::new();
    registry.push(Arc::new(PanicHook));
    registry.push(counter.clone());

    let args = json!({});
    let inv = make_invocation("noop", &args);
    let out = make_outcome("noop", &args, "ok");

    // pre-tool: panicking hook is downgraded to Continue, second hook still runs.
    let _ = registry.dispatch_pre_tool(&inv);
    registry.dispatch_post_tool(&out);

    assert_eq!(counter.pre_tool.load(Ordering::SeqCst), 1);
    assert_eq!(counter.post_tool.load(Ordering::SeqCst), 1);
}

#[test]
fn empty_registry_is_continue() {
    let registry = HookRegistry::new();
    let args = json!({});
    let inv = make_invocation("noop", &args);
    match registry.dispatch_pre_tool(&inv) {
        HookOutcome::Continue => {}
        other => panic!("expected Continue, got {:?}", other),
    }
}
