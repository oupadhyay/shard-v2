# TODO

## Direction

A full rewrite is **not** planned. The P0 rewrite question (originally at the top of this file) was resolved in favor of incremental work — see [Recently Completed](#recently-completed) for items that have already shipped, and the relevant `P1`/`P2` sections below for the items that became scoped tasks (Ollama, self-awareness, light mode).

Vercel AI SDK was evaluated and rejected: it would force the agent loop into TypeScript, losing the Rust streaming/cache/sandbox/observations stack and the entire benches suite.

## Trust Repair: Remaining Work and Test Gaps

### Ambient integration review

- [x] Import the Pierre Dark ambient shell and connected History, Saved memory,
  and Routines views. Keep the existing controller, native IPC, and scheduler.
- [x] Reject an old memory-deletion Undo after restore/change/delete. Persist the
  deletion revision and test that only the latest deletion can be undone.
- [x] Tighten the ambient views using real-content feedback: inline Settings,
  compact privacy disclosure, collapsed routine prompts, flat two-line history
  previews, capped view height, and a labelled Clear conversation menu item.
- [x] Stop showing old approval-request prose for reviewed actions. Needs
  attention starts collapsed and retains unknown outcomes without replay.
  Normal chat receives up to 20 unresolved records as read-only context, with
  oversized arguments omitted. Reduce Automatic Memory excludes this context.
- [ ] Make memory JSON and its migrated SQLite retrieval copy recover together.
  They commit separately. A retrieval-sync failure is reported after the JSON
  change has already been saved; there is no durable repair job. Do not describe
  Forget as deletion of all inferred observations or original conversations.
- [ ] Unify routine write concurrency across direct controls, approved file edits,
  and external file changes. Direct controls compare a content hash under their
  own lock; this does not serialize every writer or detect change-and-revert.
- [ ] Preserve unsaved capability-form edits when leaving a view, or ask before
  discarding them. Native hide/show retains mounted state; Back closes the view.
- [ ] Run the macOS and live-provider checklist in
  [AMBIENT_TESTING.md](./AMBIENT_TESTING.md). Linux GUI checks do not prove macOS
  focus, vibrancy, Spaces, permissions, or global shortcut behavior.
- [ ] Fix the existing strict Clippy failure in `src/vector_store.rs`
  (`chunks_exact_to_as_chunks`). Keep it separate from the ambient feature.
- [ ] Verify a packaged native build. The development app is not a release build.
- [ ] Design backend scope grants before implementing onboarding autonomy choices.
  This PR does not implement those prototype choices or a durable offline queue.

The trust repair adds review for self-changes, separate approval and execution
states, exact persona-text approval, truthful memory settings, and a **Needs
attention** section. See [TRUST_CONTRACT.md](./TRUST_CONTRACT.md) for the current
rules and test evidence. These notes do not mean the changes have been released.

### Remaining behavior and product decisions

- [ ] **Fix the rollback target at review time.** Without an `event_id`, rollback
  still selects the latest restorable edit when it runs. Store a fixed event and
  show the content to restore before approval. Test a newer edit arriving between
  review and execution.
- [ ] **Define how users resolve failed or unknown actions.** They stay visible
  and cannot retry automatically. Add a way to record that the user checked the
  effects, without changing the saved approval or pretending the action succeeded.
  Keep failure, partial effects, and an unknown outcome distinct.
- [ ] **Handle larger attention queues.** Only 100 failed/unknown actions are
  returned at once. Add navigation or filtering before claiming the view shows
  every unresolved action.
- [ ] **Track commitments beyond saved plans.** The attention view shows durable
  action plans, including unfinished steps under a completed parent. It does not
  detect every promise in chat. Define when a promise becomes a tracked commitment
  and how the user can correct or cancel it; do not infer completion from prose.
- [ ] **Decide whether to add real private sessions.** Reduce Automatic Memory
  is not private chat: history and attachments remain saved, existing conversation
  context goes to the provider, explicit memory reads remain possible, and separate
  scheduled/background/MCP activity continues. Define retention, in-flight work,
  and transitions before adding a stronger privacy claim.
- [x] **Connect the trust UI to the new ambient design.** The integration keeps
  exact-text review, unknown/failed outcomes, and access to attention items.
  Live-provider and macOS checks remain open below.

### Tests that failed or need follow-up

- [x] **Bring in the shared endpoint test fix.** A normal parallel Rust run failed
  in `agent_provider_tests::gemini_turn::http_failure_emits_error_and_returns_err`.
  The initial checkout lacked the shared lock. Before opening the PR, this branch
  was rebased onto current `main`, which includes the fix from PR #126. The normal
  parallel suite now passes. Keep this history so the earlier failure is not
  mistaken for an unresolved failure in the submitted branch.
- [ ] **Check diff-viewer test cleanup.** An intermediate frontend run reported
  asynchronous timer errors after test teardown. The final run passed; no timer
  cleanup fix was made. Confirm that repeated runs leave no pending callbacks.
- [ ] **Run native chat with live providers and tools.** The isolated GUI profile
  had no API keys. Normal streaming, cancellation during a live response, provider
  fallback, tool-call events/results, and one live external tool remain untested
  in the app. Unit tests and local HTTP mocks are not a substitute for this check.
- [ ] **Run real scheduled self-changes end to end.** Check chat and background
  proposals, persona generation, approval/rejection, and exact saved contents
  using a configured provider. Native approval checks used seeded SQLite drafts;
  generation-before-approval was checked with a local HTTP mock.
- [ ] **Test process interruption during native execution.** Storage failures,
  duplicate claims, legacy unknown rows, and no replay have automated coverage.
  Native checks covered a normal restart and seeded unknown state, not killing
  the process at each approval/write/result-save boundary. Confirm that uncertain
  effects remain unknown and are never replayed automatically.
- [ ] **Run the macOS native matrix.** Linux cannot verify NSPanel, vibrancy,
  transparency over real wallpapers, focus, global shortcuts, Spaces, or switching
  between ambient and dedicated windows on macOS.
- [ ] **Repeat native checks on the updated dependency pins.** The GUI evidence
  below was collected before the final rebase. Current `main` also brought newer
  split-crate pins; automated checks were rerun after the rebase, but the native
  GUI matrix was not repeated on those exact pins.
- [ ] **Check the normal Vite hot-reload path.** Native testing used `tauri dev`
  with the production frontend served by Vite preview. The Markdown import startup
  bug was fixed and verified there; the usual Vite development-server path still
  needs a separate native check.
- [ ] **Run the remaining release checks.** Strict Clippy, strict rustdoc, a
  packaged native build, and the full live native regression matrix were not run
  for this follow-up. Record their results before release.

### Verified in this repair

- `cargo test --workspace -- --test-threads=1`: **544 passed** before rebase;
  normal parallel `cargo test --workspace`: **544 passed** after rebase.
- `cargo check --workspace --all-targets`: **passed**. Cargo still warns that
  `screenshots v0.6.0` contains code a future Rust version will reject; the existing
  screen-capture migration task below remains relevant.
- `npm test -- --run`: **175 passed**; `npm run build`: **passed**.
- Targeted Rust formatting and `git diff --check`: **passed**. Orb setup ran twice.
- Real Linux Tauri/WebKit under Xvfb/Openbox: persona approval saved the exact
  reviewed text; rejection saved no file; an old proposal without reviewable text
  failed safely; repeat approval was refused. Failed/unknown outcomes and unfinished
  plans survived restart and appeared in both windows, without replay buttons.
- Native persona, attention, and memory-setting screenshots were inspected.
  DOM checks confirmed no horizontal overflow and that the attention section did
  not overlap the dedicated chat area. Test data used a separate temporary profile.

## Recently Completed

- ✅ **Memory system rewritten with Honcho/Hermes + RRF learnings** — see [observations.rs](../src-tauri/src/observations.rs) (peer-centric DAG, peer cards, working representation), [retrieval.rs](../src-tauri/src/retrieval.rs) (BM25 + RRF fusion), [context.rs](../src-tauri/src/context.rs) (budgeted context assembly), [tool_registry.rs](../src-tauri/src/tool_registry.rs) (Hermes-style centralized registry).
- ✅ **`agent/mod.rs` refactor** — split into 11 files (mod / process / state / types / gemini / openrouter / retry / research / schema / hash / youtube_summary). The previously-listed P2 task can be dropped.
- ✅ **Eval harness** — `cargo run --example eval --features eval` with wiremock-based endpoint overrides; see follow-ups under [P2: Eval Harness Extensions](#p2-eval-harness-extensions).

## P1: Local Models via Ollama

- [ ] Add Ollama provider for local model inference (Unsloth)
  - New `agent/ollama.rs` adapter alongside `agent/gemini.rs` / `agent/openrouter.rs`
  - Add `Provider::Ollama` to `models.rs` registry, with chat + vision entries
  - Target models on a 36 GB M3 Pro @ 128K context: **Gemma 4 26B-A4B** (`UD-Q4_K_XL`, MoE, ~18 GB) and **Qwen3.6-35B-A3B** (`UD-Q4_K_XL`, MoE, ~23 GB)
  - **Blocked**: Both Gemma 4 and Qwen3.6 unsloth GGUFs require split mmproj files; llama.cpp does not yet support the Gemma 4 architecture, and Ollama returns `500 Internal Server Error` on these models. Tracked in [ollama/ollama#15235](https://github.com/ollama/ollama/issues/15235). Revisit after the next Ollama vendor sync of llama.cpp.
  - **MTP (speculative decoding)**: Gemma 4 ships paired `-assistant` drafter models (lightweight 4-layer MTP heads) for ~2× inference speedup via speculative decoding. The drafter proposes N tokens autoregressively; the target verifies all N in one forward pass — identical output quality, significantly fewer forward passes. Currently only supported in HuggingFace Transformers; llama.cpp / Ollama do not yet support the MTP architecture. Worth waiting for MTP support before investing in local Gemma 4 inference. See [MTP docs](https://ai.google.dev/gemma/docs/mtp/mtp).

## P1: Making Shard Self-Aware

- [x] Make Shard self-aware and able to edit its own configuration. — Re-usable, allow-list-driven `read_file` + `edit_file` tools (old_str / new_str find-and-replace, like Amp). Allow-list lives in [self_files.rs](../src-tauri/src/self_files.rs); currently exposes `config.toml` only. Tool dispatch in [agent/tools/mod.rs](../src-tauri/src/agent/tools/mod.rs); registry entries in [tool_registry.rs](../src-tauri/src/tool_registry.rs); system-prompt blurb in [prompts.rs](../src-tauri/src/prompts.rs). Each edit emits a `file-edited` Tauri event ([events.ts](../src/events.ts)) carrying the structured `EditOutcome` (path / before / after / unified_diff) for the future diff viewer. Tool result also includes a fenced ` ```diff ` block that already renders in the existing tool-call accordion. API-key fields are refused defensively. Adding heartbeat specs or implementation files later is a one-line allow-list extension — no new tools.
- [ ] Allow Shard to create its own heartbeat files for scheduled tasks. (Plug into the same `edit_file` allow-list — add a `heartbeats/<name>.toml` resolver in [self_files.rs](../src-tauri/src/self_files.rs); reuse the existing diff event.)
- [ ] Maybe let Shard edit its own implementation code? (Same pattern — add `src-tauri/src/**.rs` to the allow-list with stricter guards; defer until heartbeats land.)
- [x] Show any edits via horizontal tabs (for multi-file changes) and streaming diff viewer (for a single file). — Implemented library-free in [src/ui/diff-viewer.ts](../src/ui/diff-viewer.ts) (~150 LOC, native DOM + CSS, no trees.software / diffs.com dependency). Subscribes to the backend `file-edited` event ([events.ts](../src/events.ts)), maintains a `Map<path, EditOutcome>` of recent edits, renders one horizontally-scrollable tab per path (`config.toml`, future `heartbeats/*.toml`, future `src-tauri/src/**`), and animates the unified diff in line-by-line (~120 lines/sec, fade-in keyframe). Each tab shows colored `+`/`-`/context/hunk rows, an `+N -M` stat badge, and a path bar with absolute path. Styles live at the bottom of [src/styles.css](../src/styles.css). Mounted once each in [main.ts](../src/main.ts) (ambient) and [dedicated.ts](../src/dedicated.ts) (breakout window) between the chat log and the input bar. Covered by 8 unit tests in [src/tests/diff-viewer.test.ts](../src/tests/diff-viewer.test.ts). Streaming relies on `setTimeout`-batched DOM appends so a second edit to the same path preempts any in-flight animation cleanly.

## P1: Code Refactoring

- [ ] Refactor `background.rs` (~2,500 lines) into focused modules — likely `summary.rs`, `cleanup.rs`, `deriver.rs`, `dream.rs` mirroring the four jobs.
- [ ] Refactor `heartbeat.rs` (~1,400 lines) — split spec parsing, scheduler wiring, and the draft-before-act execution loop.
- [ ] Migrate from `screenshots` crate to `xcap` for screen capture.

## P2: Eval Harness Extensions

The evaluator-as-judge harness is in place (`cargo run --example eval --features eval`) with three starter scenarios. Follow-ups:

- [ ] Expose `disable_intent_classifier` config flag so eval scenarios can pin `is_research_mode` to the configured value (currently the agent auto-promotes via LLM classification, making `research_mode: false` unreliable for prompt branch testing)
- [x] Add `seed_files:` field to scenario YAML so self-edit scenarios can pre-populate sandboxed `app_config_dir` with fixture files before the agent runs. Implemented in [examples/eval.rs](../src-tauri/examples/eval.rs) (`seed_scenario_files()`); keys are logical names matching the [self_files.rs](../src-tauri/src/self_files.rs) allow-list (currently only `config.toml`), values are paths relative to the scenarios directory. First consumer: [04_self_edit_config.yaml](../src-tauri/eval/scenarios/04_self_edit_config.yaml) with fixture under `eval/scenarios/fixtures/`. Adding a heartbeat or persona dest is one new match arm.
- [ ] Add `seed_personas:` field to scenario YAML so persona-fidelity scenarios can copy `.md` files into the sandbox `personas/` dir before agent instantiation (parallel mechanism to `seed_files:` — likely just another match arm resolving to `app_config_dir/personas/<name>.md`)
- [x] First self-edit scenario: [04_self_edit_config.yaml](../src-tauri/eval/scenarios/04_self_edit_config.yaml) — seeds a config.toml with `enable_tools = false`, asks the agent to read it, identify the model, and flip `enable_tools` to `true`; asserts `must_call_tools: [read_file, edit_file]`, `must_contain: ["enable_tools", "gpt-oss"]`, `must_not_contain: ["api_key"]`. Exercises the full read-verbatim-then-replace flow that Part 1 of [Making Shard Self-Aware](#p1-making-shard-self-aware) shipped.
- [ ] KaTeX retry scenario: prompt the agent to write a mathematical derivation and assert `agent-retry` did NOT fire (regression check for KaTeX retry trigger sensitivity)
- [ ] Empty-response retry scenario: contrived prompt that elicits reasoning-only output; assert retry triggered and recovered
- [ ] Compaction-quality scenario: 30+ turn conversation where a key fact is stated in turn 1; assert recall after compaction kicks in
- [ ] RAG relevance scenario: plant a fact via `save_memory`, ask back 20 turns later (post-compaction), verify it surfaces
- [ ] Tool-failure recovery scenario: run with bogus `BRAVE_API_KEY`; verify graceful degradation
- [ ] Persistence between turns within a scenario survives compaction trigger
- [ ] Optional: adopt `tool_choice: "none"` per-turn so scenarios can deterministically test pure reasoning paths
- [ ] CI integration: run the harness nightly with cached fixtures, post diff to PRs only on objective regressions

## P2: Light Mode Theme Support

- [ ] Add light mode CSS theme with proper color variables
- [ ] Detect system preference via `prefers-color-scheme` media query
- [ ] Add toggle in settings to override system preference
- [ ] Ensure readable unfocused fallback colors for light mode

## P2: Browser History Integration

- [ ] Add a tool to read and summarize recent browser history
  - Read Chrome/Safari history SQLite databases
  - Summarize recent browsing activity for context

## P2: Platform Gateway

- [ ] Build a gateway layer so Shard can receive/send messages via external platforms
  - Discord bot (via `serenity` or gateway API)
  - Email (IMAP polling + SMTP send)
  - SMS/iMessage (Shortcuts automation or Twilio)
  - Slack (webhook + Bolt API)
- [ ] Route inbound messages through the same `process_message()` pipeline as the chat UI
- [ ] Per-platform formatting (markdown → Discord flavored, plaintext for SMS, etc.)
- [ ] Platform-aware session management (one session per channel/thread/conversation)
- [ ] Rate limiting and authentication per platform

## P2: Sub-Agent Support

- [ ] Allow the primary agent to spawn sub-agents for parallel tool execution
  - Leverage `ToolRegistry::should_parallelize()` metadata (already exists, not yet wired)
  - Sub-agents share the same session context but run tool calls concurrently
- [ ] Orchestrator pattern: primary agent decomposes tasks, delegates to sub-agents, merges results
- [ ] Sub-agent isolation: each gets its own tool call budget and timeout
- [ ] Support for specialized sub-agents (e.g., research sub-agent with `research_mode`, code sub-agent with `run_python`)
- [ ] Progress streaming: sub-agent results streamed back to UI as they complete

## P2: Skill Auto-Creation (Procedural Memory)

- [ ] Agent automatically creates/improves personas from experience
  - After complex tasks (5+ tool calls), agent saves the working approach as a new persona
  - When user corrects the agent's approach, agent patches the relevant persona
  - `skill_manage` tool with actions: `create`, `patch`, `edit`, `delete`, `write_file`
- [ ] Track skill usage and success rate to prune stale personas
- [ ] Progressive disclosure: list names/descriptions first (~3k tokens), load full content only when needed

## P2: Multi-Provider Support

- [ ] Add support for Anthropic provider (Ollama tracked under [P1: Local Models via Ollama](#p1-local-models-via-ollama)).

## P2: Distribution & CI/CD

- [ ] Set up GitHub Actions for cross-platform builds
  - macOS: Uses existing `build-macos.sh` script
  - Auto-create releases with `.dmg`, `.msi`, `.AppImage`

## P2: Future Horizons (Documentation & Stubs)

### 2. Mobile App (iOS/Android)

- [ ] Evaluate Tauri Mobile vs React Native for the client view.
- [ ] Implement remote connection to the desktop "Shard Hub" (since the local app runs the heavy vector DB/models).
- [ ] Add share sheet extensions to quickly pipe links/text into Shard Mobile.

### 3. Nodes (Device Sync & Distributed Shard)

- [ ] Design a peer-to-peer sync protocol (e.g., libp2p or simple WebSockets) to keep `memories.sqlite` consistent across multiple devices.
- [ ] Allow one powerful desktop node to run embedded inference for weaker mobile nodes.
- [ ] Create a "Nodes" UI panel to manage connected devices and sync status.

## P3: Hybrid Retrieval Enhancements

### 1: Chunked dense retrieval for topic summaries

- [ ] Split `.md` summaries into chunks with IDs `topic::<n>`
  - Steps:
    1. Create `topics/chunks/<topic>.json` storing `{id, text, embedding}` per chunk.
    2. During recall, compute cosine over chunks and return top-k.
    3. Replace whole-topic injection with the best chunk snippets.

### 2: Pseudo-relevance feedback (PRF) for BM25

- [ ] Expand the query with top BM25 terms
  - Steps:
    1. Take top N BM25 documents; extract highest-IDF tokens.
    2. Re-run BM25 with expanded query and fuse again.
