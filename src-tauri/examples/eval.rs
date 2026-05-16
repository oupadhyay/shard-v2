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
    /// self_files allow-list (e.g. `config.toml`); values are paths to the
    /// fixture file, resolved relative to the scenarios directory.
    #[serde(default)]
    seed_files: BTreeMap<String, String>,
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

    let scenarios_dir = std::env::var("SHARD_EVAL_SCENARIOS")
        .unwrap_or_else(|_| "eval/scenarios".to_string());
    let model = std::env::var("SHARD_EVAL_MODEL")
        .unwrap_or_else(|_| "gemma-4-31b-it".to_string());
    let api_key = std::env::var("GEMINI_API_KEY")
        .map_err(|_| "GEMINI_API_KEY env var is required (.env supported)")?;

    let scenarios = load_scenarios(Path::new(&scenarios_dir))?;
    if scenarios.is_empty() {
        return Err(format!("No .yaml scenarios found in {}", scenarios_dir).into());
    }
    println!("Loaded {} scenario(s) from {}", scenarios.len(), scenarios_dir);

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

        let (objective_pass, objective_report) = evaluate_objective(&scenario, &transcript);
        let pass = objective_pass && !scenario_failed;

        write_scenario_md(&out_dir, scenario, &transcript, &objective_report, pass)?;

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
    let cfg_dir = tauri::Manager::path(handle)
        .app_config_dir()
        .map_err(|e| format!("Failed to resolve app_config_dir: {}", e))?;
    std::fs::create_dir_all(&cfg_dir)
        .map_err(|e| format!("Failed to create {}: {}", cfg_dir.display(), e))?;
    for (logical, src_rel) in &scenario.seed_files {
        // Currently the only allow-listed target is `config.toml`. Adding
        // heartbeats/*.toml here will track the same expansion in self_files.rs.
        let dst = match logical.as_str() {
            "config.toml" => cfg_dir.join("config.toml"),
            other => {
                return Err(format!(
                    "Unknown seed_files target '{}'. Allowed: config.toml",
                    other
                ));
            }
        };
        let src = scenarios_root.join(src_rel);
        let content = std::fs::read_to_string(&src)
            .map_err(|e| format!("Failed to read fixture {}: {}", src.display(), e))?;
        std::fs::write(&dst, &content)
            .map_err(|e| format!("Failed to write {}: {}", dst.display(), e))?;
        println!("  seeded {} ← {}", dst.display(), src.display());
    }
    Ok(())
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
                let result = v
                    .get("result")
                    .map(|r| r.to_string())
                    .unwrap_or_default();
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
            transcript.iter().map(|(_, t)| t.errors.len()).sum::<usize>()
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
        if objective_pass { "PASS ✓" } else { "FAIL ✗" }
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
                md.push_str(&format!(
                    "- `{}` → `{}`\n",
                    name,
                    truncate(result, 200)
                ));
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
        let name = by_id.get(id.as_str()).map(|s| s.name.as_str()).unwrap_or(id);
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
