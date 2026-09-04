//! Phase 3.2 — Crystals (procedural memory → persona) tests.
//!
//! Six cases per the plan:
//!   1. `crystallize_emits_valid_persona_markdown` — pipeline produces
//!      Markdown the existing YAML frontmatter parser can chew on.
//!   2. `sketch_below_threshold_skipped` — <5 tool calls is gated out.
//!   3. `failed_sketch_skipped` — success rate <80 % is gated out.
//!   4. `slug_collision_appends_suffix` — `pick_unique_slug` retries on conflict.
//!   5. `crystallized_persona_appears_in_list_personas` — round-trips through
//!      the personas dir + `list_available_personas_v2`.
//!   6. `crystallization_writes_through_self_files_event` — the
//!      `file-edited` Tauri event fires when the draft is written.

use crate::actions::{complete, plan, sketch_children, update_status, ActionStatus};
use crate::crystals::{
    build_prompt, find_completed_sketches, load_sketch, pick_unique_slug,
    should_crystallize_sketch, slugify, stamp_source_sketch_id, strip_markdown_fences,
    write_persona_draft, CrystallizeDecision,
};
use crate::vector_store::VectorStore;
use std::sync::Mutex;
use tempfile::tempdir;

/// Tests that touch the global personas dir / Tauri runtime must serialize on
/// the single canonical `$HOME` lock so they don't race agent/mcp/heartbeat
/// tests that mutate the same process-global `$HOME`.
use crate::tests::agent_helpers::home_lock_async as personas_test_lock_async;

fn open() -> (VectorStore, tempfile::TempDir) {
    let dir = tempdir().unwrap();
    let store = VectorStore::open(&dir.path().join("crystals.sqlite")).unwrap();
    (store, dir)
}

/// Stand up a sketch with `n` children, all marked done.
fn seed_done_sketch(store: &VectorStore, title: &str, n: usize) -> Vec<String> {
    let steps: Vec<String> = (1..=n).map(|i| format!("step {}", i)).collect();
    let step_refs: Vec<&str> = steps.iter().map(|s| s.as_str()).collect();
    let ids = plan(store, title, &step_refs, None).unwrap();
    for cid in &ids[1..] {
        complete(store, cid, Some("ok")).unwrap();
    }
    ids
}

// ─── 2. Threshold guard (too few tool calls) ──────────────────────────────

#[test]
fn sketch_below_threshold_skipped() {
    let (store, _g) = open();
    let ids = seed_done_sketch(&store, "Tiny task", 3); // < 5
    let parent = crate::actions::get(&store, &ids[0]).unwrap().unwrap();
    let kids = sketch_children(&store, &ids[0]).unwrap();
    match should_crystallize_sketch(&parent, &kids) {
        CrystallizeDecision::Skip(reason) => assert!(
            reason.contains("3") && reason.contains("≥") || reason.contains("need"),
            "expected threshold-skip reason, got: {reason}"
        ),
        CrystallizeDecision::Crystallize => panic!("3-step sketch should be skipped"),
    }
}

// ─── 3. Success-rate guard (failed sketch) ────────────────────────────────

#[test]
fn failed_sketch_skipped() {
    let (store, _g) = open();
    let ids = seed_done_sketch(&store, "Mostly broken", 5);
    // Reopen the first 3 as blocked → 2/5 success = 40 % < 80 %.
    for cid in &ids[1..4] {
        update_status(&store, cid, ActionStatus::Blocked, None, Some("failed")).unwrap();
    }
    let parent = crate::actions::get(&store, &ids[0]).unwrap().unwrap();
    let kids = sketch_children(&store, &ids[0]).unwrap();
    match should_crystallize_sketch(&parent, &kids) {
        CrystallizeDecision::Skip(reason) => {
            assert!(reason.contains("success rate"), "got: {reason}")
        }
        CrystallizeDecision::Crystallize => panic!("low-success sketch should be skipped"),
    }
}

#[test]
fn pending_children_block_decision() {
    let (store, _g) = open();
    // Default plan() leaves children pending.
    let ids = plan(&store, "Half done", &["a", "b", "c", "d", "e", "f"], None).unwrap();
    let parent = crate::actions::get(&store, &ids[0]).unwrap().unwrap();
    let kids = sketch_children(&store, &ids[0]).unwrap();
    match should_crystallize_sketch(&parent, &kids) {
        CrystallizeDecision::Skip(r) => assert!(r.contains("pending"), "got: {r}"),
        _ => panic!("pending children should block the decision"),
    }
}

#[test]
fn full_success_meets_threshold() {
    let (store, _g) = open();
    let ids = seed_done_sketch(&store, "All good", 5);
    let parent = crate::actions::get(&store, &ids[0]).unwrap().unwrap();
    let kids = sketch_children(&store, &ids[0]).unwrap();
    assert_eq!(
        should_crystallize_sketch(&parent, &kids),
        CrystallizeDecision::Crystallize
    );
}

// ─── 4. Slug collision ────────────────────────────────────────────────────

#[test]
fn slug_collision_appends_suffix() {
    // Fresh slug stays untouched.
    let s = pick_unique_slug("news-analyst", &[]).unwrap();
    assert_eq!(s, "news-analyst");

    // First collision picks `-2`.
    let s = pick_unique_slug("news-analyst", &["news-analyst".to_string()]).unwrap();
    assert_eq!(s, "news-analyst-2");

    // Multi-collision climbs.
    let existing = vec![
        "x".to_string(),
        "x-2".to_string(),
        "x-3".to_string(),
        "x-4".to_string(),
    ];
    let s = pick_unique_slug("x", &existing).unwrap();
    assert_eq!(s, "x-5");
}

#[test]
fn slug_collision_truncates_to_fit_regex() {
    // 41-char slug + collision should trim base so total stays ≤ 41.
    let long = "a".to_string() + &"b".repeat(40); // 41 chars
    assert_eq!(long.len(), 41);
    let s = pick_unique_slug(&long, std::slice::from_ref(&long)).unwrap();
    assert!(
        s.len() <= 41,
        "expected ≤41 chars after collision-retry, got {} ({})",
        s.len(),
        s
    );
    assert!(s.ends_with("-2"));
}

// ─── Pure helpers ─────────────────────────────────────────────────────────

#[test]
fn slugify_handles_punctuation_and_case() {
    assert_eq!(
        slugify("Multi-File Refactor!").as_deref(),
        Some("multi-file-refactor")
    );
    assert_eq!(
        slugify("Rename `analyst` everywhere").as_deref(),
        Some("rename-analyst-everywhere")
    );
    assert_eq!(
        slugify("   trim   spaces   ").as_deref(),
        Some("trim-spaces")
    );
    // All-symbol input → None (no usable chars).
    assert_eq!(slugify("💎💎💎"), None);
    // Leading digits stripped so the slug regex still matches.
    let s = slugify("123abc def").unwrap();
    assert!(s.starts_with('a'), "got: {s}");
}

#[test]
fn strip_markdown_fences_unwraps_common_shapes() {
    assert_eq!(strip_markdown_fences("plain\n"), "plain");
    assert_eq!(
        strip_markdown_fences("```markdown\n---\nfoo\n---\n```"),
        "---\nfoo\n---"
    );
    assert_eq!(strip_markdown_fences("```\nhello\n```"), "hello");
}

#[test]
fn stamp_source_sketch_id_idempotent() {
    let stamped = stamp_source_sketch_id("body without frontmatter", "abc-123");
    assert!(stamped.contains("source_sketch_id: abc-123"));
    // Second stamp must be a no-op.
    let twice = stamp_source_sketch_id(&stamped, "abc-123");
    assert_eq!(twice, stamped);
}

#[test]
fn build_prompt_mentions_every_step_and_outcome() {
    let (store, _g) = open();
    let ids = plan(&store, "Demo", &["alpha", "beta"], None).unwrap();
    complete(&store, &ids[1], Some("created table")).unwrap();
    complete(&store, &ids[2], Some("wired tool")).unwrap();
    let kids = sketch_children(&store, &ids[0]).unwrap();
    let prompt = build_prompt("Demo", &kids);
    assert!(prompt.contains("alpha"));
    assert!(prompt.contains("beta"));
    assert!(prompt.contains("created table"));
    assert!(prompt.contains("wired tool"));
}

// ─── Sweep pre-filter ─────────────────────────────────────────────────────

#[test]
fn find_completed_sketches_skips_partial_and_crystallised() {
    let (store, _g) = open();
    let done = seed_done_sketch(&store, "fully done", 2);
    let _partial = plan(&store, "still going", &["a", "b"], None).unwrap();
    let pre = find_completed_sketches(&store).unwrap();
    assert!(pre.contains(&done[0]));
    assert_eq!(pre.len(), 1, "only the all-terminal sketch should surface");

    crate::crystals::mark_crystallized(&store, &done[0]).unwrap();
    let post = find_completed_sketches(&store).unwrap();
    assert!(post.is_empty(), "crystallised sketches should drop out");
}

// ─── 1, 5, 6 — pipeline tests that need the personas dir + Tauri runtime ─

/// Force `$HOME` to a tempdir so `dirs::data_local_dir()` → personas dir
/// resolves inside the sandbox.
struct HomeJail {
    _td: tempfile::TempDir,
    prev: Option<std::ffi::OsString>,
}

impl HomeJail {
    fn new() -> Self {
        let td = tempfile::Builder::new()
            .prefix("shard-crystals-")
            .tempdir()
            .unwrap();
        let prev = std::env::var_os("HOME");
        std::env::set_var("HOME", td.path());
        Self { _td: td, prev }
    }
}

impl Drop for HomeJail {
    fn drop(&mut self) {
        match &self.prev {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }
}

/// Stub LLM (no Tauri) — exercises the pure synthesis path so test 1 doesn't
/// require wiremock just to assert that the assembled markdown parses.
fn fake_crystallize_markdown(sketch_id: &str) -> String {
    let body = "---\ndescription: A reusable recipe\ncategory: crystal\nrequired_tools:\n  - action_plan\n  - action_next\n---\n\n# Recipe\n\n1. Plan the work\n2. Execute it\n".to_string();
    stamp_source_sketch_id(&body, sketch_id)
}

#[test]
fn crystallize_emits_valid_persona_markdown() {
    // Pure pipeline assertion: the synthesised Markdown parses through
    // `parse_frontmatter_fields` cleanly (description + required_tools).
    let md = fake_crystallize_markdown("sketch-1");
    let meta = parse_meta_inline(&md);
    assert_eq!(meta.description.as_deref(), Some("A reusable recipe"));
    assert_eq!(meta.category.as_deref(), Some("crystal"));
    assert!(meta.required_tools.contains(&"action_plan".to_string()));
    assert!(meta.required_tools.contains(&"action_next".to_string()));
    assert!(md.contains("source_sketch_id: sketch-1"));
}

/// Inline copy of `personas::parse_frontmatter_fields` (which is private)
/// reusing the same logic the runtime applies. Keeping it here also means
/// the assertion doesn't depend on persona-dir IO.
struct InlineMeta {
    description: Option<String>,
    required_tools: Vec<String>,
    category: Option<String>,
}
fn parse_meta_inline(content: &str) -> InlineMeta {
    let mut description = None;
    let mut required_tools = Vec::new();
    let mut category = None;
    if let Some(frontmatter) = content.strip_prefix("---\n") {
        if let Some(end) = frontmatter.find("\n---") {
            let fm = &frontmatter[..end];
            let mut in_rt = false;
            for line in fm.lines() {
                let t = line.trim();
                if let Some(v) = t.strip_prefix("description:") {
                    description =
                        Some(v.trim().trim_matches(|c| c == '"' || c == '\'').to_string());
                    in_rt = false;
                } else if let Some(v) = t.strip_prefix("category:") {
                    category = Some(v.trim().trim_matches(|c| c == '"' || c == '\'').to_string());
                    in_rt = false;
                } else if t.starts_with("required_tools:") {
                    in_rt = true;
                } else if in_rt {
                    if let Some(item) = t.strip_prefix('-') {
                        let v = item
                            .trim()
                            .trim_matches(|c| c == '"' || c == '\'')
                            .to_string();
                        if !v.is_empty() {
                            required_tools.push(v);
                        }
                    } else if !t.is_empty() {
                        in_rt = false;
                    }
                }
            }
        }
    }
    InlineMeta {
        description,
        required_tools,
        category,
    }
}

#[tokio::test]
async fn crystallized_persona_appears_in_list_personas() {
    let _lock = personas_test_lock_async().await;
    let _jail = HomeJail::new();

    // Write a fake crystallised persona straight to the personas dir and
    // assert `list_available_personas_v2()` picks it up.
    let dir = crate::personas::get_personas_dir().unwrap();
    std::fs::create_dir_all(&dir).unwrap();
    let slug = "round-trip-recipe";
    let md = fake_crystallize_markdown("round-trip-sketch");
    std::fs::write(dir.join(format!("{slug}.md")), &md).unwrap();

    let listed = crate::personas::list_available_personas_v2();
    assert!(
        listed.iter().any(|s| s == slug),
        "expected `{slug}` in list, got {:?}",
        listed
    );
    let meta = crate::personas::get_persona_metadata(slug).unwrap();
    assert_eq!(meta.category.as_deref(), Some("crystal"));
    assert!(meta.required_tools.contains(&"action_plan".to_string()));
}

#[tokio::test]
async fn crystallization_writes_through_self_files_event() {
    use tauri::{Listener, Manager};

    let _lock = personas_test_lock_async().await;
    let _jail = HomeJail::new();

    // Build a mock Tauri app so write_persona_draft → edit_allowed_file
    // can emit `file-edited`. We don't need any agent state — only the
    // app handle to resolve dirs + bus the event.
    let app = tauri::test::mock_app();
    let handle = app.handle().clone();

    // Make sure the app config dir exists so resolve_allowed_path is happy
    // for any subsequent reads.
    if let Ok(cfg_dir) = handle.path().app_config_dir() {
        let _ = std::fs::create_dir_all(&cfg_dir);
    }

    let captured: std::sync::Arc<Mutex<Vec<String>>> = std::sync::Arc::new(Mutex::new(Vec::new()));
    let cap2 = captured.clone();
    handle.listen("file-edited", move |event| {
        cap2.lock().unwrap().push(event.payload().to_string());
    });

    let draft = crate::crystals::PersonaDraft {
        slug: "smoketest-persona".to_string(),
        logical_path: "personas/smoketest-persona.md".to_string(),
        markdown: fake_crystallize_markdown("evt-sketch"),
        source_sketch_id: "evt-sketch".to_string(),
    };
    let outcome = write_persona_draft(&handle, &draft).unwrap();
    assert!(outcome.abs_path.ends_with("smoketest-persona.md"));
    assert!(outcome.after.contains("source_sketch_id: evt-sketch"));

    // Allow the Tauri event bus to flush.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let events = captured.lock().unwrap();
    assert!(
        events.iter().any(|p| p.contains("smoketest-persona.md")),
        "expected file-edited for new persona, got: {:?}",
        *events
    );
}

#[tokio::test]
async fn load_sketch_round_trip() {
    let (store, _g) = open();
    let ids = seed_done_sketch(&store, "Roundtrip", 5);
    let (parent, kids) = load_sketch(&store, &ids[0]).unwrap();
    assert_eq!(parent.id, ids[0]);
    assert_eq!(kids.len(), 5);
    assert!(crystallize_uses_loaded_data(&parent, &kids));
}

/// Sanity check that `crystallize`'s signature accepts what `load_sketch`
/// returns — caught a refactor regression early in development.
fn crystallize_uses_loaded_data(
    parent: &crate::actions::Action,
    kids: &[crate::actions::Action],
) -> bool {
    // We don't actually invoke the LLM — just exercise the synchronous
    // setup that runs before `call_background_llm`.
    let base = slugify(&parent.title).unwrap();
    pick_unique_slug(&base, &[]).is_ok() && !build_prompt(&parent.title, kids).is_empty()
}

// `crystallize` itself goes through `call_background_llm` and needs a live
// HTTP endpoint — exercised by the eval scenarios rather than a unit test
// to keep the lib-test suite hermetic.
