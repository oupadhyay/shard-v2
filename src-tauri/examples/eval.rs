//! Evaluator-as-Judge Harness
//!
//! Drives the Shard agent through scripted scenarios using a real Gemini-backed
//! model (default: Gemma 4 31B), captures the resulting transcript and
//! objectively-checkable signals (tool calls, retries, errors), and writes
//! per-scenario Markdown files plus a summary that an external evaluator
//! (Claude) can grade later.
//!
//! This is intentionally NOT a unit test or benchmark:
//!   - Unit tests cover schema/serialization/tokenization/etc.
//!   - Benchmarks cover throughput.
//! This harness covers *whole-system behavior* — prompt routing, RAG-influenced
//! answers, multi-turn coherence, retry recovery, persona fidelity — none of
//! which can be asserted with simple equality checks.
//!
//! Usage:
//!   GEMINI_API_KEY=... cargo run --example eval --features eval
//!   GEMINI_API_KEY=... SHARD_EVAL_SCENARIOS=eval/scenarios cargo run --example eval --features eval
//!
//! Output: src-tauri/eval/results/<UTC-timestamp>/

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Deserialize;
use shard_lib::agent::Agent;
use shard_lib::config::AppConfig;
use tauri::Listener;

// ============================================================================
// Scenario schema (YAML)
// ============================================================================

#[derive(Debug, Deserialize, Clone)]
struct Scenario {
    /// Stable identifier (filename slug). Used for output paths.
    id: String,
    /// Human-readable scenario name.
    name: String,
    /// Free-form description of what we're evaluating.
    description: String,
    /// User turns sent in order. Each turn waits for the agent to finish.
    turns: Vec<Turn>,
    /// Objective pass/fail checks computed automatically from captured events.
    #[serde(default)]
    expectations: Expectations,
    /// Subjective rubric — passed verbatim to the Claude judge later.
    #[serde(default)]
    rubric: Vec<String>,
    /// Optional fixture files copied into the sandboxed app_config_dir BEFORE
    /// the agent runs. Keys are logical destination names matching the
    /// self_files allow-list (currently `config.toml` and `personas/<slug>.md`);
    /// values are paths to the fixture file, resolved relative to the
    /// scenarios directory.
    #[serde(default)]
    seed_files: BTreeMap<String, String>,
    /// Programmatic pre-seeding (sketches, etc.) for scenarios that need DB
    /// state the agent can't realistically build during the run.
    #[serde(default)]
    pre_seed: PreSeed,
    /// Programmatic hooks to run AFTER all turns finish but BEFORE assertions
    /// are evaluated. Lets the harness exercise pipelines (e.g. crystals
    /// queue) that don't fit into a normal user turn.
    #[serde(default)]
    post_hooks: PostHooks,
    /// SQLite-backed assertions evaluated after `post_hooks`.
    #[serde(default)]
    post_assertions: PostAssertions,
}

#[derive(Debug, Deserialize, Clone, Default)]
struct PreSeed {
    /// Pre-create completed action sketches in the actions table. The agent
    /// can later inspect them with `action_next` (frontier returns None
    /// since every child is `done`) or the crystals sweep can pick them up.
    #[serde(default)]
    completed_sketches: Vec<SketchSeed>,
}

#[derive(Debug, Deserialize, Clone)]
struct SketchSeed {
    title: String,
    /// Step titles — one child action created per entry, all marked `done`
    /// with outcome `"ok"`.
    steps: Vec<String>,
}

#[derive(Debug, Deserialize, Clone, Default)]
struct PostHooks {
    /// Bypass the real LLM-driven crystals sweep and synthesize a draft
    /// directly via `crystals::queue_synthetic_draft`. Targets the Nth seeded
    /// sketch (0-indexed) and uses `slug` as the persona slug. Exists so
    /// scenario 08 can verify the post-LLM persistence path without needing
    /// a live Groq/Gemini background-model endpoint.
    queue_synthetic_crystal: Option<SyntheticCrystalHook>,
}

#[derive(Debug, Deserialize, Clone)]
struct SyntheticCrystalHook {
    /// Index into `pre_seed.completed_sketches`.
    sketch_index: usize,
    /// Slug to use for the synthesized persona (`[a-z][a-z0-9-]{1,40}`).
    slug: String,
}

#[derive(Debug, Deserialize, Clone, Default)]
struct PostAssertions {
    /// Minimum number of rows in `proactive_queue` with `needs_approval = 1`.
    /// Scenario fails when actual count is less.
    #[serde(default)]
    proactive_queue_min_unreviewed: Option<usize>,
    /// Assert at least one row in `proactive_queue.content` contains the
    /// given substring (case-sensitive).
    #[serde(default)]
    proactive_queue_content_contains: Vec<String>,
    /// Assert every sketch row created by `pre_seed.completed_sketches` has
    /// a non-NULL `actions.crystallized_at` column.
    #[serde(default)]
    seeded_sketches_crystallized: bool,
    /// For each entry, assert the file at the logical path (resolved via
    /// the same allow-list as `seed_files`) contains the substring on disk.
    #[serde(default)]
    files_contain: BTreeMap<String, String>,
    /// For each entry, assert at least one `file_events` row exists with
    /// `event_kind = 'edit'` for the given logical path.
    #[serde(default)]
    file_events_edit_for: Vec<String>,
}

#[derive(Debug, Deserialize, Clone)]
struct Turn {
    user: String,
}

#[derive(Debug, Deserialize, Clone, Default)]
struct Expectations {
    /// Tool names that MUST be invoked (any turn) for objective pass.
    #[serde(default)]
    must_call_tools: Vec<String>,
    /// Tool names that MUST NOT be invoked.
    #[serde(default)]
    must_not_call_tools: Vec<String>,
    /// Substrings (case-insensitive) that must appear in the final assistant message.
    #[serde(default)]
    must_contain: Vec<String>,
    /// Substrings (case-insensitive) that must NOT appear.
    #[serde(default)]
    must_not_contain: Vec<String>,
    /// If true, scenario fails when `agent-error` was emitted.
    #[serde(default = "default_true")]
    no_errors: bool,
}

fn default_true() -> bool {
    true
}

// ============================================================================
// Captured per-turn signals
// ============================================================================

#[derive(Debug, Default, Clone)]
struct CapturedTurn {
    response: String,
    reasoning: String,
    tool_calls: Vec<(String, String)>, // (name, args_json)
    tool_results: Vec<(String, String)>,
    errors: Vec<String>,
    retries: Vec<String>,
}

type Captured = Arc<Mutex<CapturedTurn>>;

// ============================================================================
// Main
// ============================================================================

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Look for .env in cwd and walk upward (workspace root has it).
    dotenvy::dotenv().ok();
    if std::env::var("GEMINI_API_KEY").is_err() {
        // Try parent directory explicitly.
        if let Ok(cwd) = std::env::current_dir() {
            if let Some(parent) = cwd.parent() {
                let parent_env = parent.join(".env");
                if parent_env.exists() {
                    dotenvy::from_path(&parent_env).ok();
                }
            }
        }
    }

    let scenarios_dir =
        std::env::var("SHARD_EVAL_SCENARIOS").unwrap_or_else(|_| "eval/scenarios".to_string());
    let model = std::env::var("SHARD_EVAL_MODEL").unwrap_or_else(|_| "gemma-4-31b-it".to_string());
    let api_key = std::env::var("GEMINI_API_KEY")
        .map_err(|_| "GEMINI_API_KEY env var is required (.env supported)")?;

    // `cargo run --example eval --features eval -- --scenario <name>` lets you
    // run a single scenario without renaming the scenarios directory. Matches
    // by file stem (`07_multi_file_refactor`) or by full filename
    // (`07_multi_file_refactor.yaml`). When omitted, every YAML in the
    // scenarios dir runs.
    let scenario_filter = parse_scenario_filter();
    if let Some(ref f) = scenario_filter {
        println!("Scenario filter: {}", f);
    }

    let mut scenarios = load_scenarios(Path::new(&scenarios_dir))?;
    if let Some(ref needle) = scenario_filter {
        let stem = needle.trim_end_matches(".yaml").trim_end_matches(".yml");
        scenarios.retain(|s| s.id == stem || s.id == *needle);
    }
    if scenarios.is_empty() {
        return Err(format!(
            "No .yaml scenarios match in {} (filter: {:?})",
            scenarios_dir, scenario_filter
        )
        .into());
    }
    println!(
        "Loaded {} scenario(s) from {}",
        scenarios.len(),
        scenarios_dir
    );

    // CRITICAL: Redirect $HOME to a fresh tempdir so the agent's app_data_dir()
    // resolves into an isolated sandbox instead of the user's real
    // ~/Library/Application Support/. Without this, mock_context's empty
    // identifier causes the harness to read/write the user's actual
    // production data.
    //
    // We leak the TempDir intentionally — it must outlive every scenario, and
    // we want the directory to remain on disk for post-run inspection.
    let sandbox = Box::leak(Box::new(
        tempfile::Builder::new().prefix("shard-eval-").tempdir()?,
    ));
    std::env::set_var("HOME", sandbox.path());
    println!("Sandbox HOME = {}", sandbox.path().display());

    let timestamp = chrono::Utc::now().format("%Y-%m-%dT%H-%M-%SZ").to_string();
    let out_dir = PathBuf::from("eval/results").join(&timestamp);
    std::fs::create_dir_all(&out_dir)?;
    println!("Writing results to {}", out_dir.display());

    let mut summary_rows: Vec<(String, bool, String)> = Vec::new();

    let scenarios_root = PathBuf::from(&scenarios_dir);
    for scenario in &scenarios {
        println!("\n=== {} ({}) ===", scenario.name, scenario.id);

        // Fresh mock app per scenario → fresh memory store, fresh session.
        let app = tauri::test::mock_app();
        let handle = app.handle().clone();

        // Diagnostic: confirm we're not pointing at the user's real app_data_dir.
        // mock_context uses an empty identifier and package name "test", so this
        // should resolve to ~/Library/Application Support/test/ (or platform
        // equivalent), NOT ~/.../dev.ojasw.shard/.
        if let Ok(p) = tauri::Manager::path(&handle).app_data_dir() {
            println!("  app_data_dir = {}", p.display());
        }

        // Seed fixture files into the sandboxed app_config_dir before the
        // agent runs. This is what makes self-edit scenarios realistic:
        // read_file → copy verbatim → edit_file requires the file to exist.
        if let Err(e) = seed_scenario_files(&handle, scenario, &scenarios_root) {
            eprintln!("    seed_files error: {}", e);
        }

        // Programmatic pre-seed (e.g. completed action sketches for the
        // crystals scenario). Returns the parent ids so post-assertions can
        // inspect the rows.
        let seeded_sketch_ids = match preseed_actions(&handle, scenario) {
            Ok(ids) => ids,
            Err(e) => {
                eprintln!("    pre_seed error: {}", e);
                Vec::new()
            }
        };

        // Wire event listeners. We mutate a single CapturedTurn per turn; the
        // main loop snapshots and resets it between turns.
        let captured: Captured = Arc::new(Mutex::new(CapturedTurn::default()));
        register_listeners(&handle, captured.clone());

        let config = build_config(&model, &api_key);

        let agent = Agent::new(handle.clone());
        let mut transcript: Vec<(String, CapturedTurn)> = Vec::new();

        let mut scenario_failed = false;
        for (idx, turn) in scenario.turns.iter().enumerate() {
            println!("  turn {} → {}", idx + 1, truncate(&turn.user, 80));

            // Reset per-turn capture
            *captured.lock().unwrap() = CapturedTurn::default();

            let result = agent
                .process_message(&handle, turn.user.clone(), None, None, &config, false)
                .await;

            // Allow trailing emit() calls to flush into listeners.
            tokio::time::sleep(Duration::from_millis(150)).await;

            if let Err(e) = result {
                eprintln!("    process_message error: {}", e);
                captured.lock().unwrap().errors.push(e);
                scenario_failed = true;
            }

            let snapshot = captured.lock().unwrap().clone();
            println!(
                "    response: {} chars, tools: {}, errors: {}",
                snapshot.response.len(),
                snapshot.tool_calls.len(),
                snapshot.errors.len()
            );
            transcript.push((turn.user.clone(), snapshot));
        }

        // Post-hooks fire AFTER the turn loop, BEFORE assertions, so
        // scenario 08 can populate proactive_queue without depending on a
        // real background-LLM endpoint.
        if let Err(e) = run_post_hooks(&handle, scenario, &seeded_sketch_ids) {
            eprintln!("    post_hooks error: {}", e);
        }

        let (objective_pass, objective_report) = evaluate_objective(&scenario, &transcript);
        let (assertions_pass, assertions_report) =
            evaluate_post_assertions(&handle, scenario, &seeded_sketch_ids);
        let pass = objective_pass && assertions_pass && !scenario_failed;

        let combined_report = if assertions_report.is_empty() {
            objective_report
        } else {
            format!(
                "{}\n### Post-assertions\n{}",
                objective_report, assertions_report
            )
        };
        write_scenario_md(&out_dir, scenario, &transcript, &combined_report, pass)?;

        summary_rows.push((
            scenario.id.clone(),
            pass,
            if pass {
                "✓".to_string()
            } else {
                "✗".to_string()
            },
        ));
    }

    write_summary_md(&out_dir, &scenarios, &summary_rows)?;
    println!("\nDone. Results in {}", out_dir.display());
    Ok(())
}

// ============================================================================
// Fixture seeding
// ============================================================================

/// Copy scenario.seed_files into the sandboxed app_config_dir before the
/// agent runs. Logical destination names must match the self_files allow-list
/// (currently only `config.toml`). Source paths are resolved relative to the
/// scenarios directory so YAMLs can keep fixtures alongside themselves.
fn seed_scenario_files<R: tauri::Runtime>(
    handle: &tauri::AppHandle<R>,
    scenario: &Scenario,
    scenarios_root: &Path,
) -> Result<(), String> {
    if scenario.seed_files.is_empty() {
        return Ok(());
    }
    for (logical, src_rel) in &scenario.seed_files {
        let dst = resolve_seed_target(handle, logical)?;
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create {}: {}", parent.display(), e))?;
        }
        let src = scenarios_root.join(src_rel);
        let content = std::fs::read_to_string(&src)
            .map_err(|e| format!("Failed to read fixture {}: {}", src.display(), e))?;
        std::fs::write(&dst, &content)
            .map_err(|e| format!("Failed to write {}: {}", dst.display(), e))?;
        println!("  seeded {} ← {}", dst.display(), src.display());
    }
    Ok(())
}

/// Resolve a logical `seed_files` key to the absolute on-disk path the
/// same way `self_files::resolve_allowed_path` does (without exposing the
/// private resolver to examples). Allowed targets:
///
///   - `config.toml` → `<app_config_dir>/config.toml`
///   - `personas/<slug>.md` → `<personas_dir>/<slug>.md` (slug
///     `[a-z][a-z0-9-]{1,40}`, matches the self_files allow-list arm)
fn resolve_seed_target<R: tauri::Runtime>(
    handle: &tauri::AppHandle<R>,
    logical: &str,
) -> Result<PathBuf, String> {
    if logical == "config.toml" {
        let cfg_dir = tauri::Manager::path(handle)
            .app_config_dir()
            .map_err(|e| format!("Failed to resolve app_config_dir: {}", e))?;
        return Ok(cfg_dir.join("config.toml"));
    }
    if let Some(rest) = logical.strip_prefix("personas/") {
        let slug = rest
            .strip_suffix(".md")
            .ok_or_else(|| format!("personas/* seed target must end in .md (got '{}')", logical))?;
        shard_lib::self_files::validate_persona_slug(slug)
            .map_err(|e| format!("Invalid persona slug '{}': {}", slug, e))?;
        let personas_dir = shard_lib::personas::get_personas_dir()?;
        return Ok(personas_dir.join(format!("{}.md", slug)));
    }
    Err(format!(
        "Unknown seed_files target '{}'. Allowed: config.toml, personas/<slug>.md",
        logical
    ))
}

/// Pre-create completed action sketches. Returns parent ids in scenario order
/// so post-assertions can confirm `crystallized_at` was stamped on the right
/// rows.
fn preseed_actions<R: tauri::Runtime>(
    handle: &tauri::AppHandle<R>,
    scenario: &Scenario,
) -> Result<Vec<String>, String> {
    if scenario.pre_seed.completed_sketches.is_empty() {
        return Ok(Vec::new());
    }
    let store = shard_lib::memories::get_vector_store(handle)?;
    let mut parent_ids = Vec::new();
    for sketch in &scenario.pre_seed.completed_sketches {
        let step_refs: Vec<&str> = sketch.steps.iter().map(|s| s.as_str()).collect();
        let ids = shard_lib::actions::plan(&store, &sketch.title, &step_refs, None)?;
        // ids[0] is the parent; ids[1..] are the children.
        for cid in &ids[1..] {
            shard_lib::actions::complete(&store, cid, Some("ok"))?;
        }
        println!(
            "  pre-seeded sketch `{}` (parent={}, {} done children)",
            sketch.title,
            ids[0],
            ids.len() - 1
        );
        parent_ids.push(ids[0].clone());
    }
    Ok(parent_ids)
}

/// Programmatic post-hooks that simulate background pipelines an agent turn
/// can't drive directly.
fn run_post_hooks<R: tauri::Runtime>(
    handle: &tauri::AppHandle<R>,
    scenario: &Scenario,
    seeded_sketches: &[String],
) -> Result<(), String> {
    if let Some(ref hook) = scenario.post_hooks.queue_synthetic_crystal {
        let sketch_id = seeded_sketches
            .get(hook.sketch_index)
            .ok_or_else(|| {
                format!(
                    "queue_synthetic_crystal.sketch_index {} out of range (have {} seeded)",
                    hook.sketch_index,
                    seeded_sketches.len()
                )
            })?
            .clone();

        // Build a believable persona Markdown WITHOUT calling the LLM. The
        // YAML frontmatter mirrors what `crystallize` would synthesise so
        // downstream consumers (list_available_personas_v2,
        // get_persona_metadata) still parse it.
        let body = format!(
            "---\ndescription: Crystallised recipe from eval seeding\ncategory: crystal\nrequired_tools:\n  - action_plan\n  - action_next\n  - action_complete\n---\n\n# Recipe\n\n1. Plan the work\n2. Execute it\n3. Verify the outcome\n"
        );
        let markdown = shard_lib::crystals::stamp_source_sketch_id(&body, &sketch_id);
        let draft = shard_lib::crystals::PersonaDraft {
            slug: hook.slug.clone(),
            logical_path: format!("personas/{}.md", hook.slug),
            markdown,
            source_sketch_id: sketch_id.clone(),
        };
        shard_lib::crystals::queue_synthetic_draft(handle, &draft)?;
        println!(
            "  post-hook: queued synthetic crystal `{}` for sketch {}",
            hook.slug, sketch_id
        );
    }
    Ok(())
}

/// Evaluate the post-run SQLite assertions. Mirrors `evaluate_objective`'s
/// (pass, markdown_report) return convention so they merge cleanly.
fn evaluate_post_assertions<R: tauri::Runtime>(
    handle: &tauri::AppHandle<R>,
    scenario: &Scenario,
    seeded_sketches: &[String],
) -> (bool, String) {
    let pa = &scenario.post_assertions;
    let nothing_to_check = pa.proactive_queue_min_unreviewed.is_none()
        && pa.proactive_queue_content_contains.is_empty()
        && !pa.seeded_sketches_crystallized
        && pa.files_contain.is_empty()
        && pa.file_events_edit_for.is_empty();
    if nothing_to_check {
        return (true, String::new());
    }

    let mut report = String::new();
    let mut all_pass = true;

    let store = match shard_lib::memories::get_vector_store(handle) {
        Ok(s) => s,
        Err(e) => {
            return (
                false,
                format!("- [ ] post_assertions could not open vector store: {}\n", e),
            );
        }
    };

    // Pull every unreviewed message once and reuse for both queue-related
    // checks. `get_unreviewed_messages` is the public surface heartbeat
    // exposes; querying proactive_queue through it keeps us out of the
    // VectorStore's private `conn` field.
    let _ = shard_lib::heartbeat::ensure_proactive_queue_table(handle);
    let unreviewed = shard_lib::heartbeat::get_unreviewed_messages(handle, 256).unwrap_or_default();

    if let Some(min) = pa.proactive_queue_min_unreviewed {
        let count = unreviewed.iter().filter(|m| m.needs_approval).count();
        let ok = count >= min;
        report.push_str(&format!(
            "- [{}] proactive_queue_min_unreviewed: expected ≥{}, got {}\n",
            if ok { "x" } else { " " },
            min,
            count
        ));
        if !ok {
            all_pass = false;
        }
    }

    for needle in &pa.proactive_queue_content_contains {
        let ok = unreviewed.iter().any(|m| m.content.contains(needle));
        report.push_str(&format!(
            "- [{}] proactive_queue_content_contains {:?}\n",
            if ok { "x" } else { " " },
            needle
        ));
        if !ok {
            all_pass = false;
        }
    }

    if pa.seeded_sketches_crystallized {
        if seeded_sketches.is_empty() {
            report.push_str("- [ ] seeded_sketches_crystallized: no sketches were pre-seeded\n");
            all_pass = false;
        } else {
            for id in seeded_sketches {
                let crystallized =
                    shard_lib::crystals::is_crystallized(&store, id).unwrap_or(false);
                report.push_str(&format!(
                    "- [{}] actions.crystallized_at set for sketch `{}`\n",
                    if crystallized { "x" } else { " " },
                    id,
                ));
                if !crystallized {
                    all_pass = false;
                }
            }
        }
    }

    for (logical, needle) in &pa.files_contain {
        let resolved = match resolve_seed_target(handle, logical) {
            Ok(p) => p,
            Err(e) => {
                report.push_str(&format!(
                    "- [ ] files_contain {:?}: resolve failed: {}\n",
                    logical, e
                ));
                all_pass = false;
                continue;
            }
        };
        match std::fs::read_to_string(&resolved) {
            Ok(content) => {
                let ok = content.contains(needle);
                report.push_str(&format!(
                    "- [{}] {} contains {:?}\n",
                    if ok { "x" } else { " " },
                    logical,
                    needle
                ));
                if !ok {
                    all_pass = false;
                }
            }
            Err(e) => {
                report.push_str(&format!(
                    "- [ ] files_contain {:?}: read failed: {}\n",
                    logical, e
                ));
                all_pass = false;
            }
        }
    }

    for logical in &pa.file_events_edit_for {
        let events = shard_lib::file_history::get_events(&store, logical, 50).unwrap_or_default();
        let count = events
            .iter()
            .filter(|e| matches!(e.event_kind, shard_lib::file_history::FileEventKind::Edit))
            .count();
        let ok = count > 0;
        report.push_str(&format!(
            "- [{}] file_events has ≥1 edit row for `{}` (got {})\n",
            if ok { "x" } else { " " },
            logical,
            count
        ));
        if !ok {
            all_pass = false;
        }
    }

    (all_pass, report)
}

/// `--scenario <name>` argv parser. Accepts either `--scenario X` or
/// `--scenario=X`. Returns None when omitted.
fn parse_scenario_filter() -> Option<String> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if let Some(v) = arg.strip_prefix("--scenario=") {
            return Some(v.to_string());
        }
        if arg == "--scenario" {
            return args.next();
        }
    }
    None
}

// ============================================================================
// Listener wiring
// ============================================================================

fn register_listeners<R: tauri::Runtime>(handle: &tauri::AppHandle<R>, cap: Captured) {
    {
        let cap = cap.clone();
        handle.listen("agent-response-chunk", move |e| {
            // Payload is a JSON-encoded string; the Tauri SDK quotes it.
            let payload = strip_json_string_quotes(e.payload());
            cap.lock().unwrap().response.push_str(&payload);
        });
    }
    {
        let cap = cap.clone();
        handle.listen("agent-reasoning-chunk", move |e| {
            let payload = strip_json_string_quotes(e.payload());
            cap.lock().unwrap().reasoning.push_str(&payload);
        });
    }
    {
        let cap = cap.clone();
        handle.listen("agent-tool-call", move |e| {
            let payload = e.payload().to_string();
            // Outer layer is a JSON-encoded string containing JSON; unwrap once.
            let inner = serde_json::from_str::<String>(&payload).unwrap_or(payload);
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&inner) {
                let name = v
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("?")
                    .to_string();
                let args = v
                    .get("args")
                    .map(|a| a.to_string())
                    .unwrap_or_else(|| "{}".to_string());
                cap.lock().unwrap().tool_calls.push((name, args));
            }
        });
    }
    {
        let cap = cap.clone();
        handle.listen("agent-tool-result", move |e| {
            let payload = e.payload().to_string();
            let inner = serde_json::from_str::<String>(&payload).unwrap_or(payload);
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&inner) {
                let name = v
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("?")
                    .to_string();
                let result = v.get("result").map(|r| r.to_string()).unwrap_or_default();
                cap.lock().unwrap().tool_results.push((name, result));
            }
        });
    }
    {
        let cap = cap.clone();
        handle.listen("agent-error", move |e| {
            cap.lock()
                .unwrap()
                .errors
                .push(strip_json_string_quotes(e.payload()));
        });
    }
    {
        handle.listen("agent-retry", move |e| {
            cap.lock()
                .unwrap()
                .retries
                .push(strip_json_string_quotes(e.payload()));
        });
    }
}

fn strip_json_string_quotes(payload: &str) -> String {
    serde_json::from_str::<String>(payload).unwrap_or_else(|_| payload.to_string())
}

// ============================================================================
// Config builder
// ============================================================================

fn build_config(model: &str, gemini_key: &str) -> AppConfig {
    AppConfig {
        api_key: None,
        gemini_api_key: Some(gemini_key.to_string()),
        openrouter_api_key: None,
        brave_api_key: std::env::var("BRAVE_API_KEY")
            .or_else(|_| std::env::var("BRAVE_SEARCH_API_KEY"))
            .ok(),
        groq_api_key: None,
        selected_model: Some(model.to_string()),
        api_base_url: None,
        enable_web_search: Some(true),
        enable_tools: Some(true),
        system_prompt: None,
        incognito_mode: Some(false),
        research_mode: Some(false),
        background_model: None,
        max_auto_retries: Some(2),
        retry_on_empty: Some(true),
        retry_on_katex: Some(true),
        enable_screen_context: Some(false),
        enable_compaction: Some(true),
        compaction_threshold: Some(0.5),
        compaction_preserve_turns: Some(5),
        fallback_model: None,
        heartbeat_global_cooldown_secs: None,
    }
}

// ============================================================================
// Scenario loading
// ============================================================================

fn load_scenarios(dir: &Path) -> Result<Vec<Scenario>, Box<dyn std::error::Error>> {
    let mut out = Vec::new();
    if !dir.exists() {
        return Err(format!("Scenarios directory not found: {}", dir.display()).into());
    }
    let mut entries: Vec<_> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .map(|ext| ext == "yaml" || ext == "yml")
                .unwrap_or(false)
        })
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let content = std::fs::read_to_string(entry.path())?;
        let s: Scenario = serde_yaml::from_str(&content)
            .map_err(|e| format!("Failed to parse {:?}: {}", entry.path(), e))?;
        out.push(s);
    }
    Ok(out)
}

// ============================================================================
// Objective evaluation
// ============================================================================

fn evaluate_objective(
    scenario: &Scenario,
    transcript: &[(String, CapturedTurn)],
) -> (bool, String) {
    let mut report = String::new();
    let mut all_pass = true;

    let all_tool_calls: Vec<&str> = transcript
        .iter()
        .flat_map(|(_, t)| t.tool_calls.iter().map(|(n, _)| n.as_str()))
        .collect();
    // Concatenate ALL assistant responses across turns so substring checks
    // succeed when the relevant fact appears in any turn (not just the last).
    let all_responses: String = transcript
        .iter()
        .map(|(_, t)| t.response.to_lowercase())
        .collect::<Vec<_>>()
        .join("\n");
    let any_errors = transcript.iter().any(|(_, t)| !t.errors.is_empty());

    for required in &scenario.expectations.must_call_tools {
        let ok = all_tool_calls.iter().any(|t| t == required);
        report.push_str(&format!(
            "- [{}] must_call_tool `{}`\n",
            if ok { "x" } else { " " },
            required
        ));
        if !ok {
            all_pass = false;
        }
    }
    for forbidden in &scenario.expectations.must_not_call_tools {
        let bad = all_tool_calls.iter().any(|t| t == forbidden);
        report.push_str(&format!(
            "- [{}] must_not_call_tool `{}`\n",
            if !bad { "x" } else { " " },
            forbidden
        ));
        if bad {
            all_pass = false;
        }
    }
    for needle in &scenario.expectations.must_contain {
        let ok = all_responses.contains(&needle.to_lowercase());
        report.push_str(&format!(
            "- [{}] must_contain {:?} (any turn)\n",
            if ok { "x" } else { " " },
            needle
        ));
        if !ok {
            all_pass = false;
        }
    }
    for needle in &scenario.expectations.must_not_contain {
        let bad = all_responses.contains(&needle.to_lowercase());
        report.push_str(&format!(
            "- [{}] must_not_contain {:?}\n",
            if !bad { "x" } else { " " },
            needle
        ));
        if bad {
            all_pass = false;
        }
    }
    if scenario.expectations.no_errors {
        report.push_str(&format!(
            "- [{}] no_errors (got {} error event(s))\n",
            if !any_errors { "x" } else { " " },
            transcript
                .iter()
                .map(|(_, t)| t.errors.len())
                .sum::<usize>()
        ));
        if any_errors {
            all_pass = false;
        }
    }
    (all_pass, report)
}

// ============================================================================
// Output writers
// ============================================================================

fn write_scenario_md(
    dir: &Path,
    scenario: &Scenario,
    transcript: &[(String, CapturedTurn)],
    objective_report: &str,
    objective_pass: bool,
) -> std::io::Result<()> {
    let mut md = String::new();
    md.push_str(&format!("# {}\n\n", scenario.name));
    md.push_str(&format!("**ID:** `{}`\n\n", scenario.id));
    md.push_str(&format!("**Description:** {}\n\n", scenario.description));
    md.push_str(&format!(
        "**Objective verdict:** {}\n\n",
        if objective_pass {
            "PASS ✓"
        } else {
            "FAIL ✗"
        }
    ));

    md.push_str("## Transcript\n\n");
    for (i, (user, t)) in transcript.iter().enumerate() {
        md.push_str(&format!("### Turn {}\n\n", i + 1));
        md.push_str(&format!("**User:** {}\n\n", user));
        if !t.reasoning.is_empty() {
            md.push_str(&format!(
                "<details><summary>Reasoning ({} chars)</summary>\n\n```\n{}\n```\n\n</details>\n\n",
                t.reasoning.len(),
                t.reasoning
            ));
        }
        if !t.tool_calls.is_empty() {
            md.push_str("**Tool calls:**\n\n");
            for (name, args) in &t.tool_calls {
                md.push_str(&format!("- `{}` args=`{}`\n", name, truncate(args, 200)));
            }
            md.push('\n');
        }
        if !t.tool_results.is_empty() {
            md.push_str("**Tool results:**\n\n");
            for (name, result) in &t.tool_results {
                md.push_str(&format!("- `{}` → `{}`\n", name, truncate(result, 200)));
            }
            md.push('\n');
        }
        md.push_str(&format!(
            "**Assistant:**\n\n{}\n\n",
            if t.response.is_empty() {
                "_(empty)_".to_string()
            } else {
                t.response.clone()
            }
        ));
        if !t.errors.is_empty() {
            md.push_str("**Errors:**\n\n");
            for e in &t.errors {
                md.push_str(&format!("- {}\n", e));
            }
            md.push('\n');
        }
        if !t.retries.is_empty() {
            md.push_str(&format!("**Retries triggered:** {}\n\n", t.retries.len()));
        }
    }

    md.push_str("## Objective Checks\n\n");
    if objective_report.is_empty() {
        md.push_str("_(no objective checks declared)_\n\n");
    } else {
        md.push_str(objective_report);
        md.push('\n');
    }

    md.push_str("## Subjective Rubric (for Claude judge)\n\n");
    if scenario.rubric.is_empty() {
        md.push_str("_(no rubric declared)_\n\n");
    } else {
        for r in &scenario.rubric {
            md.push_str(&format!("- {}\n", r));
        }
        md.push('\n');
    }
    md.push_str("**Judge instructions:** For each rubric item, output `PASS` / `FAIL` / `PARTIAL` with one-sentence justification grounded in the transcript above.\n");

    let path = dir.join(format!("{}.md", scenario.id));
    std::fs::write(path, md)
}

fn write_summary_md(
    dir: &Path,
    scenarios: &[Scenario],
    rows: &[(String, bool, String)],
) -> std::io::Result<()> {
    let mut by_id: BTreeMap<&str, &Scenario> = BTreeMap::new();
    for s in scenarios {
        by_id.insert(&s.id, s);
    }
    let mut md = String::new();
    md.push_str("# Eval Run Summary\n\n");
    md.push_str(&format!(
        "Run at `{}` — {} scenario(s)\n\n",
        chrono::Utc::now().to_rfc3339(),
        scenarios.len()
    ));
    md.push_str("| Scenario | Objective | Subjective (judge) |\n");
    md.push_str("|----------|-----------|---------------------|\n");
    for (id, _pass, marker) in rows {
        let name = by_id
            .get(id.as_str())
            .map(|s| s.name.as_str())
            .unwrap_or(id);
        md.push_str(&format!("| {} ({}) | {} | _pending_ |\n", name, id, marker));
    }
    md.push_str(
        "\n## Judge prompt\n\n\
         Read each `<id>.md` file in this directory. For each scenario, fill the\n\
         _Subjective (judge)_ column with `PASS` / `FAIL` / `PARTIAL` per rubric\n\
         item, then add a one-paragraph qualitative summary at the bottom.\n",
    );
    std::fs::write(dir.join("SUMMARY.md"), md)
}

// ============================================================================
// Helpers
// ============================================================================

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}
