# TODO

## P0: Code Review Issues (Feb 2026)

- [x] **Monolithic handleInput** ([main.ts:69-224](../src/main.ts)) - Split into `ChatController` class in `src/chat.ts`

## P1: Auto-Testing (Evaluator-as-a-Judge)

- [ ] Automate actual testing with the model using an **Evaluator-as-a-Judge** pattern:
  1. **Automated UI**: Use **Playwright** or **Cypress** with the Tauri WebDriver to drive the frontend.
  2. **Synthetic User**: Script a separate LLM (e.g., GPT-4o or a local Llama instance) to generate prompts, send them to Shard, and wait for the UI to update.
  3. **Verification**: Have the Evaluator LLM check the final DOM state or your `interactions.jsonl` against a set of "ground truth" requirements.
  4. **Mocking**: Use a test flag to swap real tool calls (like `web_search`) with static JSON mocks to keep tests deterministic and save your quota.

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
- [ ] Update models supported (especially with OpenRouter free model router)

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

- [ ] Add support for other providers (e.g., Ollama, Anthropic).

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
