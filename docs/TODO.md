# TODO

## P0: Code Review Issues (Feb 2026)

- [x] **Monolithic handleInput** ([src/chat.ts](../src/chat.ts)) - Split into `ChatController` class in `src/chat.ts`

## P0: Local Models via Ollama

- [ ] Add Ollama provider for local model inference
  - New `agent/ollama.rs` adapter alongside `agent/gemini.rs` / `agent/openrouter.rs`
  - Add `Provider::Ollama` to `models.rs` registry, with chat + vision entries
  - Target models on a 36 GB M3 Pro @ 128K context: **Gemma 4 26B-A4B** (`UD-Q4_K_XL`, MoE, ~18 GB) and **Qwen3.6-35B-A3B** (`UD-Q4_K_XL`, MoE, ~23 GB)
  - **Blocked**: Both Gemma 4 and Qwen3.6 unsloth GGUFs require split mmproj files; llama.cpp does not yet support the Gemma 4 architecture, and Ollama returns `500 Internal Server Error` on these models. Tracked in [ollama/ollama#15235](https://github.com/ollama/ollama/issues/15235). Revisit after the next Ollama vendor sync of llama.cpp.

## P1: Evaluate Nemotron 3 Super 120B

- [ ] Evaluate `nvidia/nemotron-3-super-120b-a12b:free` as a potential replacement for GPT-OSS 120B (OpenRouter fallback)
  - Hybrid Mamba-Transformer MoE architecture, 262K context window
  - Reportedly excellent multi-step agent coherence and long-horizon planning
  - **Validate:** Tool calling reliability with Shard's 17-tool schema (especially multi-turn tool chains)
  - **Validate:** Structured output quality for background jobs (JSON parsing, memory extraction)
  - If validated, replace `openai/gpt-oss-120b:free` as the default `fallback_model`

## P2: Eval Harness Extensions

The evaluator-as-judge harness is in place (`cargo run --example eval --features eval`) with three starter scenarios. Follow-ups:

- [ ] Expose `disable_intent_classifier` config flag so eval scenarios can pin `is_research_mode` to the configured value (currently the agent auto-promotes via LLM classification, making `research_mode: false` unreliable for prompt branch testing)
- [ ] Add `seed_personas:` field to scenario YAML so persona-fidelity scenarios can copy `.md` files into the sandbox `personas/` dir before agent instantiation
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

## P2: Code Refactoring

- [ ] Refactor `agent/mod.rs` (1300+ lines) into smaller modules
  - Split provider-specific logic: `agent/gemini_provider.rs`, `agent/openrouter_provider.rs`
  - Extract core logic: `agent/core.rs`
  - Consider separating retry logic, tool execution, and streaming handling
- [ ] Migrate from `screenshots` crate to `xcap` for screen capture (P2)
- [ ] Rationalize model registry per [model strategy proposal](./model_strategy_proposal.md): remove Cerebras, add Gemma 4 26B-A4B MoE for vision/background, cut from 22 → 10 entries

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

- [ ] Add support for Anthropic provider (Ollama tracked under P0 above).

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
