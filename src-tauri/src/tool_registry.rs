//! Hermes-style Tool Registry
//!
//! Centralizes tool metadata (schema, toolset, caching, parallelism) in one place.
//! Replaces the flat `Vec<ToolDefinition>` in `tools.rs` with a queryable registry
//! that supports Hermes-style toolset grouping, parallel-safety hints, and
//! per-tool cache TTL configuration.

use crate::agent::{FunctionDefinition, ToolDefinition};
use serde_json::json;
use std::collections::{HashMap, HashSet};

static REGISTRY: std::sync::OnceLock<ToolRegistry> = std::sync::OnceLock::new();

/// Returns a reference to the global singleton `ToolRegistry`.
pub fn global() -> &'static ToolRegistry {
    REGISTRY.get_or_init(ToolRegistry::new)
}

// ============================================================================
// ToolEntry — Hermes-style metadata per tool
// ============================================================================

/// Metadata for a single registered tool.
/// Mirrors Hermes Agent's `ToolEntry` but adapted for Rust/Shard.
pub struct ToolEntry {
    /// Unique tool name (matches the function schema name)
    pub name: &'static str,
    /// Toolset grouping (e.g., "web", "memory", "persona", "research", "code", "automation")
    pub toolset: &'static str,
    /// OpenAI-compatible function schema
    pub schema: ToolDefinition,
    /// Whether this tool is safe to run concurrently with other parallel-safe tools.
    /// Mirrors Hermes `_PARALLEL_SAFE_TOOLS`.
    pub parallel_safe: bool,
    /// Cache TTL in seconds, or None if results should never be cached.
    pub cache_ttl_secs: Option<u64>,
    /// Whether this tool requires draft approval during heartbeat/cron execution.
    pub draft_gated: bool,
}

// ============================================================================
// ToolRegistry
// ============================================================================

/// Central registry for all agent tools.
/// Built once at startup; queried per-turn for available tool definitions.
pub struct ToolRegistry {
    tools: HashMap<&'static str, ToolEntry>,
}

/// Tools that are always available regardless of active personas.
const GLOBAL_TOOLS: &[&str] = &[
    "web_search",
    "open_url",
    "memory_search",
    "memory_get",
    "save_memory",
    "refresh_memories",
    "read_topic_summary",
    "update_topic_summary",
    "load_persona",
    "unload_persona",
    "list_personas",
    "search_wikipedia",
    "youtube_transcript",
    "run_python",
    "wake_me_up_in",
    // Self-awareness: read + edit Shard's own files (allow-listed)
    "read_file",
    "edit_file",
    "file_history",
    "rollback_self_edit",
    // Phase 3.1 — Action / Frontier planner
    "action_plan",
    "action_next",
    "action_complete",
    "action_block",
];

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolRegistry {
    /// Build the complete registry with all known tools.
    pub fn new() -> Self {
        let mut tools = HashMap::new();

        // Helper macro to reduce boilerplate
        macro_rules! register {
            ($name:expr, $toolset:expr, $desc:expr, $params:expr,
             parallel: $parallel:expr, cache_ttl: $ttl:expr, draft: $draft:expr,
             strict: $strict:expr) => {
                tools.insert(
                    $name,
                    ToolEntry {
                        name: $name,
                        toolset: $toolset,
                        schema: ToolDefinition {
                            tool_type: "function".to_string(),
                            function: FunctionDefinition {
                                name: $name.to_string(),
                                description: $desc.to_string(),
                                parameters: $params,
                                strict: $strict,
                            },
                        },
                        parallel_safe: $parallel,
                        cache_ttl_secs: $ttl,
                        draft_gated: $draft,
                    },
                );
            };
        }

        // ── Web toolset ──────────────────────────────────────────────────
        register!(
            "web_search", "web",
            "Search the web for current/recent information. BEST for: sports scores, news, current events, live data, recent updates. Returns 5 results with title, URL, and snippet. One search is usually sufficient - avoid multiple redundant searches.",
            json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Search query. Be specific: include year, team name, 'current', 'latest', or 'today' for time-sensitive queries." }
                },
                "required": ["query"],
                "additionalProperties": false
            }),
            parallel: true, cache_ttl: Some(7 * 24 * 60 * 60), draft: false, strict: Some(true)
        );

        register!(
            "open_url", "web",
            "Read the main text content of any URL/web page and provides clean, readable text. Use this to read specific articles or pages found via web_search or directly requested by the user.",
            json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "The full HTTPS URL of the web page to read." }
                },
                "required": ["url"],
                "additionalProperties": false
            }),
            parallel: true, cache_ttl: None, draft: false, strict: Some(true)
        );

        register!(
            "search_wikipedia", "web",
            "Search Wikipedia for encyclopedic/historical information. Best for background knowledge, biographies, and established facts. NOT for current events, live scores, or breaking news.",
            json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Wikipedia article title. Use exact page title as it appears on Wikipedia (e.g., 'San Francisco 49ers', 'Albert Einstein'). For example, use 'SchedMD' and 'NVIDIA' not 'SchedMD acquisition by NVIDIA'" }
                },
                "required": ["query"],
                "additionalProperties": false
            }),
            parallel: true, cache_ttl: Some(7 * 24 * 60 * 60), draft: false, strict: Some(true)
        );

        register!(
            "youtube_transcript", "web",
            "Get the transcript/captions of a YouTube video. Accepts a YouTube URL or video ID. Returns timestamped text. Use this when the user asks about the content of a YouTube video, wants a summary, or references a YouTube link.",
            json!({
                "type": "object",
                "properties": {
                    "video": { "type": "string", "description": "YouTube video URL (e.g. 'https://www.youtube.com/watch?v=dQw4w9WgXcQ') or video ID (e.g. 'dQw4w9WgXcQ')" }
                },
                "required": ["video"],
                "additionalProperties": false
            }),
            parallel: true, cache_ttl: Some(60 * 24 * 60 * 60), draft: false, strict: Some(true)
        );

        // ── Research toolset ─────────────────────────────────────────────
        register!(
            "search_arxiv", "research",
            "Search ArXiv for academic papers and preprints. Best for scientific research, AI/ML papers, physics, math. Returns paper titles, authors, and abstracts.",
            json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Academic search query, e.g. 'transformer attention mechanism' or 'quantum computing'" }
                },
                "required": ["query"],
                "additionalProperties": false
            }),
            parallel: true, cache_ttl: Some(7 * 24 * 60 * 60), draft: false, strict: Some(true)
        );

        register!(
            "read_arxiv_paper", "research",
            "Read the full content of an ArXiv paper. Use this AFTER search_arxiv to get detailed paper content. Input can be ArXiv paper ID (e.g., '2401.12345') or URL.",
            json!({
                "type": "object",
                "properties": {
                    "paper_id": { "type": "string", "description": "ArXiv paper ID (e.g., '2401.12345') or full arxiv.org URL" }
                },
                "required": ["paper_id"],
                "additionalProperties": false
            }),
            parallel: true, cache_ttl: Some(7 * 24 * 60 * 60), draft: false, strict: Some(true)
        );

        // ── Finance toolset ──────────────────────────────────────────────
        register!(
            "get_stock_price", "finance",
            "Get current stock price and basic financial data for a ticker symbol. Returns price, change, and volume.",
            json!({
                "type": "object",
                "properties": {
                    "symbol": { "type": "string", "description": "Stock ticker symbol, e.g. AAPL, GOOGL, MSFT" }
                },
                "required": ["symbol"],
                "additionalProperties": false
            }),
            parallel: true, cache_ttl: Some(60 * 60), draft: false, strict: Some(true)
        );

        // ── Weather toolset ──────────────────────────────────────────────
        register!(
            "get_weather", "weather",
            "Get current weather for a location. Returns temperature, conditions, and humidity.",
            json!({
                "type": "object",
                "properties": {
                    "location": { "type": "string", "description": "City name ONLY (e.g. 'Paris', 'London', 'Tracy'). Do NOT include state abbreviations, country codes, or commas." }
                },
                "required": ["location"],
                "additionalProperties": false
            }),
            parallel: true, cache_ttl: Some(60 * 60), draft: false, strict: Some(true)
        );

        // ── Memory toolset ───────────────────────────────────────────────
        register!(
            "save_memory", "memory",
            "Save important user preferences, context, or facts to persistent memory. Use for genuinely persistent information. Call when: user explicitly requests you remember something, user states a strong preference (language, units, coding style), or user provides important project context for ongoing work.",
            json!({
                "type": "object",
                "properties": {
                    "category": {
                        "type": "string",
                        "enum": ["preference", "project", "interaction", "fact"],
                        "description": "Category of memory: 'preference' for user preferences, 'project' for project context, 'interaction' for conversation summaries, 'fact' for general facts about the user"
                    },
                    "content": { "type": "string", "description": "The information to remember. Be concise but complete." },
                    "importance": { "type": "integer", "description": "Importance level 1-5 (5=critical, 1=nice-to-have)" }
                },
                "required": ["category", "content", "importance"],
                "additionalProperties": false
            }),
            parallel: false, cache_ttl: None, draft: false, strict: Some(true)
        );

        register!(
            "update_topic_summary", "memory",
            "Create or update a focused summary file for a specific topic or project (e.g., 'SHARD', 'FINANCE'). Use this to consolidate scattered information into a single coherent document. IMPORTANT: Always use read_topic_summary first to get the existing content before updating.",
            json!({
                "type": "object",
                "properties": {
                    "topic": { "type": "string", "description": "Topic name (e.g., 'SHARD', 'FINANCE'). Will be used as filename (SHARD.md)." },
                    "content": { "type": "string", "description": "The full markdown content of the summary. This overwrites the existing file, so ensure you include all relevant previous information plus new updates." }
                },
                "required": ["topic", "content"],
                "additionalProperties": false
            }),
            parallel: false, cache_ttl: None, draft: false, strict: Some(true)
        );

        register!(
            "read_topic_summary", "memory",
            "Read the content of an existing topic summary file. Use this before updating a summary to ensure you don't overwrite existing information.",
            json!({
                "type": "object",
                "properties": {
                    "topic": { "type": "string", "description": "Topic name (e.g., 'SHARD', 'FINANCE')." }
                },
                "required": ["topic"],
                "additionalProperties": false
            }),
            parallel: true, cache_ttl: None, draft: false, strict: Some(true)
        );

        register!(
            "refresh_memories", "memory",
            "Run background memory analysis to update topic summaries and insights from recent conversations. Use only when the user explicitly requests a memory update AND there is significant new information to remember.",
            json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            parallel: false, cache_ttl: None, draft: false, strict: Some(true)
        );

        register!(
            "memory_search", "memory",
            "Search your persistent memory (topics, insights, session transcripts) using semantic and keyword search. Returns ranked snippets with source paths and line ranges. Can use temporal filters for chats.",
            json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Natural language search query describing what to find in memory" },
                    "max_results": { "type": ["integer", "null"], "description": "Maximum number of results to return (default: 5)" },
                    "min_score": { "type": ["number", "null"], "description": "Minimum similarity score 0.0-1.0 (default: 0.3)." },
                    "time_filter": { "type": ["string", "null"], "description": "Optional: 'last_conversation', 'yesterday', 'last_week', or specific YYYY-MM-DD date" }
                },
                "required": ["query", "max_results", "min_score", "time_filter"],
                "additionalProperties": false
            }),
            parallel: true, cache_ttl: None, draft: false, strict: Some(true)
        );

        register!(
            "memory_get", "memory",
            "Read specific lines from a memory file or full session transcript. Provide either 'path' (for topics/insights) or 'session_id' (for full chats).",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": ["string", "null"], "description": "Relative path to the memory file (e.g. 'topics/SHARD.md')" },
                    "session_id": { "type": ["string", "null"], "description": "Session UUID to fetch the full transcript." },
                    "from": { "type": ["integer", "null"], "description": "Starting line number, 1-indexed (default: 1)" },
                    "lines": { "type": ["integer", "null"], "description": "Number of lines to read (default: 50, max: 200)" }
                },
                "required": ["path", "session_id", "from", "lines"],
                "additionalProperties": false
            }),
            parallel: true, cache_ttl: None, draft: false, strict: Some(true)
        );

        // ── Persona toolset ──────────────────────────────────────────────
        register!(
            "load_persona", "persona",
            "Load a specific dynamic persona into your session context. This adds specialized instructions to your system prompt. You should ONLY load a persona if it is strictly necessary to answer the user's current prompt. You MUST unload the persona when the specific mini-task is complete to avoid context pollution. Please check the 'Available Personas' section in your system prompt to see what personas you can load.",
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "The exact name of the persona to load" }
                },
                "required": ["name"],
                "additionalProperties": false
            }),
            parallel: false, cache_ttl: None, draft: false, strict: Some(true)
        );

        register!(
            "unload_persona", "persona",
            "Unload a specific dynamic persona from your session context. You MUST do this immediately after you have finished the specific mini-task that required the persona.",
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "The name of the persona to unload" }
                },
                "required": ["name"],
                "additionalProperties": false
            }),
            parallel: false, cache_ttl: None, draft: false, strict: Some(true)
        );

        register!(
            "list_personas", "persona",
            "List all dynamically loadable personas available in the workspace. Use this to discover expertise you can adopt.",
            json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            parallel: true, cache_ttl: None, draft: false, strict: Some(true)
        );

        // ── Code toolset ─────────────────────────────────────────────────
        register!(
            "run_python", "code",
            "Execute Python code in a sandboxed environment and return the output. Use for calculations, data processing, generating text, or any task that benefits from running code. The sandbox has no persistent filesystem — each execution starts fresh. Print results to stdout.",
            json!({
                "type": "object",
                "properties": {
                    "code": {
                        "type": "string",
                        "description": "Python 3 source code to execute. Use print() to produce output."
                    }
                },
                "required": ["code"],
                "additionalProperties": false
            }),
            parallel: false, cache_ttl: None, draft: false, strict: Some(true)
        );

        // ── Automation toolset ───────────────────────────────────────────
        register!(
            "wake_me_up_in", "automation",
            "Set a one-shot timer that triggers a heartbeat-like callback after the specified duration. Use this to schedule follow-up checks, reminders, or delayed actions. The context string is passed back as the prompt when the timer fires.",
            json!({
                "type": "object",
                "properties": {
                    "duration_minutes": {
                        "type": "integer",
                        "description": "Number of minutes to wait before triggering the callback. Minimum 1, maximum 1440 (24 hours)."
                    },
                    "context": {
                        "type": "string",
                        "description": "Context/prompt to pass when the timer fires. Include enough detail to resume the task."
                    }
                },
                "required": ["duration_minutes", "context"],
                "additionalProperties": false
            }),
            parallel: false, cache_ttl: None, draft: false, strict: Some(true)
        );

        // ── Self-awareness: read an allow-listed file ────────────────────
        register!(
            "read_file", "automation",
            "Read the contents of an allow-listed Shard file (e.g. 'config.toml'). Returns the file contents verbatim. API-key fields are stripped on save and never present in config.toml. Call this BEFORE edit_file so you know the exact current text — `edit_file` requires an exact `old_str` substring match.",
            json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Allow-listed file path. Currently allowed: 'config.toml' (Shard's runtime configuration)."
                    }
                },
                "required": ["path"],
                "additionalProperties": false
            }),
            parallel: true, cache_ttl: None, draft: false, strict: Some(true)
        );

        // ── Self-awareness: edit an allow-listed file ────────────────────
        register!(
            "edit_file", "automation",
            "Edit an allow-listed Shard file by replacing `old_str` with `new_str`. Currently allow-listed: 'config.toml' only. `old_str` MUST be an exact substring of the file (whitespace included) and unique unless `replace_all=true`. Returns a unified diff of the change; the frontend renders this as a diff viewer. Refuses any edit that touches API-key fields.",
            json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Allow-listed file path. Currently allowed: 'config.toml'."
                    },
                    "old_str": {
                        "type": "string",
                        "description": "Exact text to replace. Must occur verbatim in the file. If empty and the file is empty, the file is created with `new_str`."
                    },
                    "new_str": {
                        "type": "string",
                        "description": "Replacement text. May be empty (to delete `old_str`)."
                    },
                    "replace_all": {
                        "type": "boolean",
                        "description": "If true, replace every occurrence of `old_str`. Default false; in that case `old_str` must be unique."
                    }
                },
                "required": ["path", "old_str", "new_str", "replace_all"],
                "additionalProperties": false
            }),
            parallel: false, cache_ttl: None, draft: false, strict: Some(true)
        );

        // ── Self-awareness: file history (read-only, safe) ───────────────
        register!(
            "file_history", "automation",
            "Return prior read/edit/revert/snapshot events for an allow-listed Shard file. Call this BEFORE editing a non-trivial file so you can see prior diffs, edit cadence, and whether earlier edits were followed by tool errors. Returns Markdown with a one-line summary, optional ⚠️ caution if prior edits caused errors, and the most recent events (with diffs).",
            json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Allow-listed file path (same names as read_file/edit_file). Currently: 'config.toml'."
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum events to return (default 10, max 50). Most recent first."
                    }
                },
                "required": ["path", "limit"],
                "additionalProperties": false
            }),
            parallel: true, cache_ttl: None, draft: false, strict: Some(true)
        );

        // ── Phase 3.1 — Action / Frontier planner ────────────────────────
        register!(
            "action_plan", "automation",
            "Create a multi-step plan ('sketch') for a complex task. Inserts a parent action plus N children chained in order (each step depends on the previous). Returns all action ids. Call this when a task needs >1 self-edit or tool call across distinct phases — `action_next` then walks the chain, surviving compaction.",
            json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "Sketch title (the goal)." },
                    "steps": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Ordered list of step titles. Each will be inserted as a pending child action."
                    }
                },
                "required": ["title", "steps"],
                "additionalProperties": false
            }),
            parallel: false, cache_ttl: None, draft: false, strict: Some(true)
        );

        register!(
            "action_next", "automation",
            "Return the next ready action (highest priority, all dependencies done). Returns null if the queue is empty or every pending action is blocked. Call this at the top of a turn when an open sketch exists — it survives compaction so the agent can resume a refactor mid-flight.",
            json!({
                "type": "object",
                "properties": {},
                "required": [],
                "additionalProperties": false
            }),
            parallel: true, cache_ttl: None, draft: false, strict: Some(true)
        );

        register!(
            "action_complete", "automation",
            "Mark an action as done and record a brief outcome. The action's dependents become eligible for `action_next` on the next call.",
            json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Action id (from action_plan or action_next)." },
                    "outcome": { "type": "string", "description": "One-line summary of what was done." }
                },
                "required": ["id", "outcome"],
                "additionalProperties": false
            }),
            parallel: false, cache_ttl: None, draft: false, strict: Some(true)
        );

        register!(
            "action_block", "automation",
            "Mark an action as blocked with a reason. Removes it from the frontier until explicitly re-opened.",
            json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Action id." },
                    "reason": { "type": "string", "description": "Why this is blocked (e.g. 'awaiting user clarification')." }
                },
                "required": ["id", "reason"],
                "additionalProperties": false
            }),
            parallel: false, cache_ttl: None, draft: false, strict: Some(true)
        );

        // ── Self-awareness: rollback (restorative, draft-gated for cron) ─
        register!(
            "rollback_self_edit", "automation",
            "Restore an allow-listed file to its pre-edit state. Looks up the most recent restorable edit for `path` in file_events (or the specific `event_id` if supplied) and writes the stored snapshot back. Records a `revert` event for auditability. Use this when a recent edit caused a tool error you can see in file_history. Returns the reverted event id and new file length.",
            json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Allow-listed file path (same names as read_file/edit_file). Currently: 'config.toml'."
                    },
                    "event_id": {
                        "type": "string",
                        "description": "Optional specific file_events.id to roll back to. When omitted, rolls back the most recent restorable edit."
                    }
                },
                "required": ["path", "event_id"],
                "additionalProperties": false
            }),
            parallel: false, cache_ttl: None, draft: true, strict: Some(true)
        );

        // ── Phase 3.2 — Crystals (meta-persona only, draft-gated) ────────
        register!(
            "crystallize_sketch", "automation",
            "Turn a completed action sketch into a reusable Markdown persona under `personas/<slug>.md`. Pulls the parent + children, asks the background LLM to summarise the recipe, and writes the result via the self-edit allow-list. Draft-gated — the call serializes into proactive_queue for user approval before the persona lands on disk. Persona-gated to the `meta` persona; not visible in normal chat.",
            json!({
                "type": "object",
                "properties": {
                    "sketch_id": { "type": "string", "description": "Parent action id of the sketch to crystallise (returned by `action_plan`)." }
                },
                "required": ["sketch_id"],
                "additionalProperties": false
            }),
            parallel: false, cache_ttl: None, draft: true, strict: Some(true)
        );

        // ── Heartbeat-only (draft-gated) tools ───────────────────────────

        register!(
            "create_heartbeat", "automation",
            "Create a new heartbeat spec file. Heartbeats are autonomous scheduled tasks. This creates a new .toml file in the heartbeats directory. Requires user approval.",
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Filename for the heartbeat (without .toml extension)" },
                    "schedule": { "type": "string", "description": "Cron expression (e.g. '0 */2 * * *' for every 2 hours)" },
                    "session": { "type": "string", "description": "Session namespace (e.g. 'agent:my-task')" },
                    "prompt": { "type": "string", "description": "The prompt/instructions for the heartbeat" },
                    "persona": { "type": "string", "description": "Optional persona to load for this heartbeat" },
                    "max_tool_calls": { "type": "integer", "description": "Optional max tool calls per run (default: 5)" }
                },
                "required": ["name", "schedule", "session", "prompt"],
                "additionalProperties": false
            }),
            parallel: false, cache_ttl: None, draft: true, strict: None
        );

        register!(
            "delete_heartbeat", "automation",
            "Delete an existing heartbeat spec file. Permanently removes the scheduled task. Requires user approval.",
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Filename of the heartbeat to delete (without .toml extension)" }
                },
                "required": ["name"],
                "additionalProperties": false
            }),
            parallel: false, cache_ttl: None, draft: true, strict: Some(true)
        );

        register!(
            "edit_heartbeat", "automation",
            "Edit an existing heartbeat spec's schedule, prompt, persona, or other fields. Requires user approval.",
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Filename of the heartbeat to edit (without .toml extension)" },
                    "schedule": { "type": "string", "description": "New cron expression (optional)" },
                    "prompt": { "type": "string", "description": "New prompt body (optional)" },
                    "persona": { "type": "string", "description": "New persona (optional, empty string to clear)" },
                    "max_tool_calls": { "type": "integer", "description": "New max tool calls (optional)" }
                },
                "required": ["name"],
                "additionalProperties": false
            }),
            parallel: false, cache_ttl: None, draft: true, strict: None
        );

        Self { tools }
    }

    /// Get filtered tool definitions for an agent turn.
    ///
    /// Mirrors Hermes `get_definitions()`: returns schemas for tools that are
    /// either in the global set or required by an active persona.
    pub fn get_definitions(&self, active_personas: &[String]) -> Vec<ToolDefinition> {
        let required: HashSet<String> = active_personas
            .iter()
            .flat_map(|p| crate::personas::get_persona_required_tools(p))
            .collect();

        let mut defs: Vec<_> = self.tools
            .values()
            .filter(|e| {
                let is_global = GLOBAL_TOOLS.contains(&e.name);
                let is_required = required.contains(e.name);
                // Draft-gated tools are NOT included in normal chat definitions
                let is_draft = e.draft_gated;
                (is_global || is_required) && !is_draft
            })
            .map(|e| e.schema.clone())
            .collect();
        defs.sort_by(|a, b| a.function.name.cmp(&b.function.name));
        defs
    }

    /// Get tool definitions for heartbeat/cron runs (includes draft-gated tools).
    pub fn get_heartbeat_definitions(&self, active_personas: &[String]) -> Vec<ToolDefinition> {
        let required: HashSet<String> = active_personas
            .iter()
            .flat_map(|p| crate::personas::get_persona_required_tools(p))
            .collect();

        let mut defs: Vec<_> = self.tools
            .values()
            .filter(|e| {
                let is_global = GLOBAL_TOOLS.contains(&e.name);
                let is_required = required.contains(e.name);
                let is_draft = e.draft_gated;
                is_global || is_required || is_draft
            })
            .map(|e| e.schema.clone())
            .collect();
        defs.sort_by(|a, b| a.function.name.cmp(&b.function.name));
        defs
    }

    /// Look up a tool entry by name.
    pub fn get(&self, name: &str) -> Option<&ToolEntry> {
        self.tools.get(name)
    }

    /// Get the cache TTL for a tool, if it should be cached.
    /// Replaces `cache::get_ttl_for_tool()`.
    pub fn cache_ttl(&self, name: &str) -> Option<u64> {
        self.tools.get(name).and_then(|e| e.cache_ttl_secs)
    }

    /// Check if a tool requires draft approval (heartbeat mode).
    /// Replaces `tools::is_draft_gated()`.
    pub fn is_draft_gated(&self, name: &str) -> bool {
        self.tools
            .get(name)
            .is_some_and(|e| e.draft_gated)
    }

    /// Hermes-style: can this batch of tool calls run in parallel?
    pub fn should_parallelize(&self, tool_names: &[&str]) -> bool {
        tool_names
            .iter()
            .all(|n| self.tools.get(*n).is_some_and(|e| e.parallel_safe))
    }

    /// Get all tool names in a given toolset.
    pub fn tools_in_toolset(&self, toolset: &str) -> Vec<&'static str> {
        self.tools
            .values()
            .filter(|e| e.toolset == toolset)
            .map(|e| e.name)
            .collect()
    }

    /// List all unique toolset names.
    pub fn toolsets(&self) -> Vec<&'static str> {
        let mut sets: Vec<&str> = self
            .tools
            .values()
            .map(|e| e.toolset)
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        sets.sort();
        sets
    }
}
