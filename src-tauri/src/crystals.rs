//! Phase 3.2 — Crystals: turn a completed action sketch into a reusable
//! persona Markdown file.
//!
//! Pipeline:
//!
//!   1. Background sweep picks up sketches whose every child is terminal
//!      (done/cancelled/blocked) and whose tool-call success rate ≥ 80 %
//!      across ≥ 5 steps.
//!   2. `crystallize()` collects the parent + children, builds an LLM prompt
//!      describing the recipe, and asks the configured background model to
//!      return Markdown for a reusable persona.
//!   3. The result is written through [`crate::self_files::edit_allowed_file`]
//!      under `personas/<slug>.md`, which routes through the existing
//!      file_events / diff-viewer pipeline. The background sweep gates the
//!      actual write behind a `proactive_queue` draft so the user approves
//!      before the persona lands on disk.
//!
//! Most of the logic in this module is intentionally pure (slug generation,
//! decision predicate, prompt building, markdown sanitisation) so the LLM
//! step can be mocked cleanly in tests.
//!
//! The on-disk persona is tagged with `source_sketch_id` in YAML frontmatter
//! so the sweep can dedupe even before we mark the action row as
//! crystallised, and so users can trace a persona back to the recipe that
//! created it.

use serde::{Deserialize, Serialize};

use crate::actions::{Action, ActionStatus};
use crate::self_files::{validate_persona_slug, EditOutcome};
use crate::vector_store::VectorStore;

/// Minimum number of non-cancelled children for a sketch to be eligible.
pub const MIN_TOOL_CALLS: usize = 5;
/// Required fraction of `done` children among non-cancelled ones.
pub const MIN_SUCCESS_RATE: f32 = 0.80;
/// Maximum slug suffix we'll try before giving up on collision resolution.
pub const MAX_SLUG_COLLISION_RETRIES: usize = 50;

/// One crystallised persona — returned from [`crystallize`] before the
/// caller decides whether to write it directly or stash it in the
/// proactive_queue for user approval.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PersonaDraft {
    /// Validated `[a-z][a-z0-9-]{1,40}` slug (without `.md`).
    pub slug: String,
    /// Logical path passed to `self_files::edit_allowed_file`.
    /// Always `format!("personas/{}.md", slug)`.
    pub logical_path: String,
    /// Full Markdown content with YAML frontmatter.
    pub markdown: String,
    /// Sketch parent action id this draft was derived from.
    pub source_sketch_id: String,
}

/// Decision returned by [`should_crystallize_sketch`]. `Skip` carries a
/// human-readable reason so the background sweep can log it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CrystallizeDecision {
    Crystallize,
    Skip(String),
}

/// Pure predicate: should we turn this completed sketch into a persona?
///
/// Requires:
///   - Every child terminal (no `pending`/`active`).
///   - `non_cancelled = done + blocked` ≥ [`MIN_TOOL_CALLS`].
///   - `done / non_cancelled` ≥ [`MIN_SUCCESS_RATE`].
///
/// A parent's own status is ignored — agents tend to ack the parent the
/// moment they pick up a sketch, so the parent says little about the run.
pub fn should_crystallize_sketch(_parent: &Action, children: &[Action]) -> CrystallizeDecision {
    if children.is_empty() {
        return CrystallizeDecision::Skip("no children".to_string());
    }
    let mut done = 0usize;
    let mut blocked = 0usize;
    for c in children {
        match c.status {
            ActionStatus::Done => done += 1,
            ActionStatus::Blocked => blocked += 1,
            ActionStatus::Cancelled => {}
            ActionStatus::Pending | ActionStatus::Active => {
                return CrystallizeDecision::Skip(format!(
                    "child `{}` is still {}",
                    c.title,
                    c.status.as_str()
                ));
            }
        }
    }
    let non_cancelled = done + blocked;
    if non_cancelled < MIN_TOOL_CALLS {
        return CrystallizeDecision::Skip(format!(
            "only {}/{} tool calls (need ≥{})",
            non_cancelled,
            MIN_TOOL_CALLS,
            MIN_TOOL_CALLS
        ));
    }
    let rate = done as f32 / non_cancelled as f32;
    if rate < MIN_SUCCESS_RATE {
        return CrystallizeDecision::Skip(format!(
            "success rate {:.0}% < {:.0}%",
            rate * 100.0,
            MIN_SUCCESS_RATE * 100.0
        ));
    }
    CrystallizeDecision::Crystallize
}

/// Convert a free-form title into a persona slug matching
/// `[a-z][a-z0-9-]{1,40}`. Returns `None` only if the input contains no
/// usable characters at all (e.g. `"💎"` → `None`).
pub fn slugify(title: &str) -> Option<String> {
    let lowered = title.to_lowercase();
    let mut out = String::with_capacity(lowered.len());
    let mut prev_dash = true; // suppress leading dash
    for c in lowered.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    // Trim trailing dash.
    while out.ends_with('-') {
        out.pop();
    }
    // First char must be a letter — strip any leading digits.
    while out
        .chars()
        .next()
        .map(|c| !c.is_ascii_lowercase())
        .unwrap_or(false)
    {
        out.remove(0);
    }
    // Hard-cap at 41 chars so the validator accepts it.
    if out.len() > 41 {
        out.truncate(41);
        while out.ends_with('-') {
            out.pop();
        }
    }
    if out.len() < 2 {
        return None;
    }
    Some(out)
}

/// Resolve a slug collision by appending `-2`, `-3`, … against the list of
/// already-taken slugs (typically `list_available_personas()`). Returns the
/// base unchanged when free.
pub fn pick_unique_slug(base: &str, existing: &[String]) -> Result<String, String> {
    if !existing.iter().any(|s| s == base) {
        return Ok(base.to_string());
    }
    for n in 2..=MAX_SLUG_COLLISION_RETRIES {
        // Reserve room for the suffix so we don't exceed the slug regex.
        let suffix = format!("-{}", n);
        let max_base = 41usize.saturating_sub(suffix.len());
        let trimmed: String = base.chars().take(max_base).collect();
        let trimmed = trimmed.trim_end_matches('-').to_string();
        let candidate = format!("{}{}", trimmed, suffix);
        if validate_persona_slug(&candidate).is_ok() && !existing.iter().any(|s| s == &candidate) {
            return Ok(candidate);
        }
    }
    Err(format!(
        "could not find a free persona slug derived from `{}` within {} attempts",
        base, MAX_SLUG_COLLISION_RETRIES
    ))
}

/// Build the prompt the background LLM gets asked to crystallise. Kept pure
/// so tests can assert on the input the model would see.
pub fn build_prompt(parent_title: &str, children: &[Action]) -> String {
    let mut prompt = String::new();
    prompt.push_str("You are a procedural-memory crystalliser. ");
    prompt.push_str("Given a completed action sketch (a parent task and its child steps with outcomes), ");
    prompt.push_str("produce a reusable Markdown persona that captures the recipe so a future agent ");
    prompt.push_str("can replay it.\n\n");
    prompt.push_str("Return ONLY valid Markdown beginning with a YAML frontmatter block of the form:\n\n");
    prompt.push_str("```\n---\n");
    prompt.push_str("description: <one sentence, ≤120 chars>\n");
    prompt.push_str("category: crystal\n");
    prompt.push_str("required_tools:\n  - <tool_name>\n  - ...\n");
    prompt.push_str("---\n```\n\n");
    prompt.push_str("Followed by a numbered list of the steps, each annotated with what worked.\n\n");
    prompt.push_str("## Sketch\n\n");
    prompt.push_str(&format!("Title: {}\n\n", parent_title));
    prompt.push_str("## Steps\n\n");
    for (i, c) in children.iter().enumerate() {
        let status = c.status.as_str();
        let outcome = c.outcome.as_deref().unwrap_or("(no outcome recorded)");
        prompt.push_str(&format!(
            "{}. [{}] {}\n   outcome: {}\n",
            i + 1,
            status,
            c.title,
            outcome
        ));
    }
    prompt.push('\n');
    prompt
}

/// Strip the common ```markdown / ``` code fences some models wrap responses in.
/// Returns the inner text unchanged when no fence is present.
pub fn strip_markdown_fences(raw: &str) -> String {
    let trimmed = raw.trim();
    if let Some(rest) = trimmed
        .strip_prefix("```markdown")
        .or_else(|| trimmed.strip_prefix("```md"))
        .or_else(|| trimmed.strip_prefix("```"))
    {
        if let Some(inner) = rest.trim_start_matches('\n').strip_suffix("```") {
            return inner.trim_end().to_string();
        }
    }
    trimmed.to_string()
}

/// Inject a `source_sketch_id` line into existing YAML frontmatter — or add
/// frontmatter if the model forgot it. Idempotent.
pub fn stamp_source_sketch_id(markdown: &str, sketch_id: &str) -> String {
    let needle = format!("source_sketch_id: {}", sketch_id);
    if markdown.contains(&needle) {
        return markdown.to_string();
    }
    if markdown.starts_with("---\n") {
        // Insert just after the opening `---`.
        let mut out = String::from("---\n");
        out.push_str(&needle);
        out.push('\n');
        out.push_str(&markdown[4..]);
        return out;
    }
    // Wrap with minimal frontmatter.
    format!(
        "---\n{}\ncategory: crystal\n---\n\n{}",
        needle,
        markdown.trim_start()
    )
}

/// Collect the sketch parent + its children from the store. Synchronous
/// wrapper around two queries — kept separate from [`crystallize`] so the
/// LLM-calling step never holds a non-`Send` `VectorStore` across an await.
pub fn load_sketch(
    store: &VectorStore,
    sketch_id: &str,
) -> Result<(Action, Vec<Action>), String> {
    let parent = crate::actions::get(store, sketch_id)?
        .ok_or_else(|| format!("sketch {} not found", sketch_id))?;
    if parent.parent_id.is_some() {
        return Err(format!(
            "action {} is a child (parent={:?}), not a sketch root",
            sketch_id, parent.parent_id
        ));
    }
    let children = crate::actions::sketch_children(store, sketch_id)?;
    if children.is_empty() {
        return Err(format!("sketch {} has no children", sketch_id));
    }
    Ok((parent, children))
}

/// Synthesize a draft from an already-loaded sketch by calling the
/// configured background LLM. Pure data in, pure data out — does NOT write
/// to disk; the caller decides whether to send the draft straight through
/// `edit_allowed_file` or queue it as a `proactive_queue` draft first.
///
/// Caller must have already established that `should_crystallize_sketch`
/// returned `Crystallize`.
pub async fn crystallize(
    http_client: &reqwest::Client,
    config: &crate::config::AppConfig,
    parent: &Action,
    children: &[Action],
    existing_slugs: &[String],
) -> Result<PersonaDraft, String> {
    let base_slug = slugify(&parent.title)
        .ok_or_else(|| format!("could not derive slug from title `{}`", parent.title))?;
    let slug = pick_unique_slug(&base_slug, existing_slugs)?;

    let prompt = build_prompt(&parent.title, children);
    let model = config
        .background_model
        .as_deref()
        .unwrap_or(crate::background::DEFAULT_BACKGROUND_MODEL);

    let raw = crate::background::call_background_llm(http_client, config, model, &prompt).await?;
    let stripped = strip_markdown_fences(&raw);
    let markdown = stamp_source_sketch_id(&stripped, &parent.id);

    Ok(PersonaDraft {
        slug: slug.clone(),
        logical_path: format!("personas/{}.md", slug),
        markdown,
        source_sketch_id: parent.id.clone(),
    })
}

/// Write a [`PersonaDraft`] straight to disk via the existing self-edit
/// allow-list. Routes through `edit_allowed_file` so it lands in
/// `file_events`, then emits the `file-edited` Tauri event the diff viewer
/// listens for. Mirrors what the `edit_file` tool dispatch does for the
/// agent's normal self-edits.
pub fn write_persona_draft<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    draft: &PersonaDraft,
) -> Result<EditOutcome, String> {
    use tauri::Emitter;
    // Empty `old_str` against an empty file creates fresh content — matches
    // the unit-tested `apply_edit_empty_to_empty_file_creates_content` case.
    let outcome = crate::self_files::edit_allowed_file(
        app_handle,
        &draft.logical_path,
        "",
        &draft.markdown,
        false,
    )?;
    let _ = app_handle.emit("file-edited", &outcome);
    Ok(outcome)
}

/// Mark an action row as having been crystallised. Best-effort — failure
/// only means a future sweep might re-process the same sketch (the slug
/// uniqueness check will then catch the dup).
pub fn mark_crystallized(store: &VectorStore, sketch_id: &str) -> Result<(), String> {
    let now = chrono::Utc::now().to_rfc3339();
    store
        .conn
        .execute(
            "UPDATE actions SET crystallized_at = ? WHERE id = ?",
            rusqlite::params![now, sketch_id],
        )
        .map_err(|e| format!("mark_crystallized failed: {}", e))?;
    Ok(())
}

/// Has this sketch already been crystallised (via the column flag)?
pub fn is_crystallized(store: &VectorStore, sketch_id: &str) -> Result<bool, String> {
    use rusqlite::OptionalExtension;
    let flag: Option<Option<String>> = store
        .conn
        .query_row(
            "SELECT crystallized_at FROM actions WHERE id = ?",
            rusqlite::params![sketch_id],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(|e| format!("is_crystallized lookup failed: {}", e))?;
    Ok(flag.flatten().is_some())
}

/// Return parent ids of every sketch whose children are all terminal and
/// which has not yet been crystallised. Cheap pre-filter for the periodic
/// sweep — full predicate runs in [`should_crystallize_sketch`].
pub fn find_completed_sketches(store: &VectorStore) -> Result<Vec<String>, String> {
    let mut stmt = store
        .conn
        .prepare(
            "SELECT id FROM actions \
             WHERE parent_id IS NULL AND crystallized_at IS NULL \
             ORDER BY updated_at DESC",
        )
        .map_err(|e| e.to_string())?;
    let roots: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?
        .filter_map(|r| r.ok())
        .collect();

    let mut completed = Vec::new();
    for root in roots {
        let children = crate::actions::sketch_children(store, &root)?;
        if children.is_empty() {
            continue;
        }
        let all_terminal = children.iter().all(|c| c.status.is_terminal()
            || matches!(c.status, ActionStatus::Blocked));
        if all_terminal {
            completed.push(root);
        }
    }
    Ok(completed)
}

/// Periodic sweep: for every completed sketch eligible by the threshold,
/// crystallise and queue the resulting persona as a `proactive_queue`
/// draft so the user can approve it before it lands on disk.
///
/// Returns the number of drafts queued.
pub async fn sweep_and_queue_drafts<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
) -> Result<usize, String> {
    crate::heartbeat::ensure_proactive_queue_table(app_handle)?;
    let config = crate::config::load_config(app_handle)?;
    let http_client = reqwest::Client::new();
    let existing_slugs = crate::personas::list_available_personas();

    // Collect everything synchronously first so we never hold a `&VectorStore`
    // (non-`Send`) across an LLM await.
    let eligible: Vec<(Action, Vec<Action>)> = {
        let store = crate::memories::get_vector_store(app_handle)?;
        let mut out = Vec::new();
        for sketch_id in find_completed_sketches(&store)? {
            let parent = match crate::actions::get(&store, &sketch_id)? {
                Some(p) => p,
                None => continue,
            };
            let children = crate::actions::sketch_children(&store, &sketch_id)?;
            match should_crystallize_sketch(&parent, &children) {
                CrystallizeDecision::Skip(reason) => {
                    log::debug!(
                        "[Crystals] Skip sketch {} ({}): {}",
                        sketch_id,
                        parent.title,
                        reason
                    );
                    continue;
                }
                CrystallizeDecision::Crystallize => out.push((parent, children)),
            }
        }
        out
    };

    let mut queued = 0usize;
    for (parent, children) in eligible {
        let sketch_id = parent.id.clone();
        match crystallize(&http_client, &config, &parent, &children, &existing_slugs).await {
            Ok(draft) => {
                queue_draft(app_handle, &draft)?;
                if let Ok(store) = crate::memories::get_vector_store(app_handle) {
                    let _ = mark_crystallized(&store, &sketch_id);
                }
                queued += 1;
                log::info!(
                    "[Crystals] Queued draft `{}` from sketch {}",
                    draft.slug,
                    sketch_id
                );
            }
            Err(e) => {
                log::warn!(
                    "[Crystals] Failed to crystallise sketch {}: {}",
                    sketch_id,
                    e
                );
            }
        }
    }
    Ok(queued)
}

/// Test-friendly bypass: stash a fully-formed [`PersonaDraft`] in the
/// `proactive_queue` and stamp the source sketch's `crystallized_at` column,
/// skipping the background LLM call. Used by the eval harness (and any
/// future integration test) that wants to exercise the post-synthesis
/// persistence pipeline without requiring a real LLM endpoint.
///
/// In production this is what `sweep_and_queue_drafts` does after the LLM
/// returns, minus the LLM call itself.
pub fn queue_synthetic_draft<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    draft: &PersonaDraft,
) -> Result<(), String> {
    crate::heartbeat::ensure_proactive_queue_table(app_handle)?;
    queue_draft(app_handle, draft)?;
    let store = crate::memories::get_vector_store(app_handle)?;
    mark_crystallized(&store, &draft.source_sketch_id)
}

/// Stash a draft persona in the proactive_queue. The serialized payload
/// matches the existing draft-act schema so the same approve/reject flow
/// the heartbeat engine uses applies here.
fn queue_draft<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    draft: &PersonaDraft,
) -> Result<(), String> {
    let payload = serde_json::json!({
        "name": "crystallize_sketch",
        "arguments": {
            "logical_path": draft.logical_path,
            "markdown": draft.markdown,
            "source_sketch_id": draft.source_sketch_id,
        },
        "justification": format!(
            "Crystallise sketch {} into a reusable persona `{}`.",
            draft.source_sketch_id, draft.slug
        ),
        "heartbeat_session": "crystals:sweep",
    });
    let content = format!(
        "**Crystals sweep proposes persona `{}`** from sketch `{}`.\n\nPreview:\n\n```markdown\n{}\n```",
        draft.slug,
        draft.source_sketch_id,
        preview(&draft.markdown, 600)
    );
    let msg = crate::heartbeat::ProactiveMessage {
        id: uuid::Uuid::new_v4().to_string(),
        heartbeat_session: "crystals:sweep".to_string(),
        content,
        draft_payload: Some(payload.to_string()),
        needs_approval: true,
        reviewed_at: None,
        approved: None,
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    crate::heartbeat::insert_proactive_message(app_handle, &msg)
}

fn preview(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}
