# TODO

## P0: Code Review Issues (Feb 2026)

- [ ] **Manual config merging** - (Deferred) Using `#[serde(default)]` patterns, full refactor to `figment` deemed too large for this pass.
- [x] **Global frontend state** ([main.ts:42-48](../src/main.ts)) - Encapsulated in `ChatState` class (`src/state.ts`)
- [ ] **Monolithic handleInput** ([main.ts:69-224](../src/main.ts)) - Split into `preparePayload`, `sendChatMessage`, etc.

## P0: OpenClaw Gaps

- [x] **Unified Session Model** - Isolate chat contexts instead of a single global append-only history.
- [x] **Skills Engine & Pi Runtime** - Expose skills as discoverable "tools" (e.g., `list_skills`, `load_skill`) so the agent can temporarily assume personas or load instructions on-demand, rather than forcing them into the global system prompt.
- [x] **Automation (Cronjobs, Webhooks, Gmail)** - Background tasks and event-driven agent runs.

## P1: Better Screen Context Experience

- [ ] Instead of showing analyzing screen, show no loading and just animate when the suggestions are ready (especially useful when the user just opens the chat window to ask a question unrelated to screen context). This will make the screen context experience feel faster (since no loading state) and more intuitive.

## P1: LaTeX/Markdown Error Detection UI

- [ ] **Unbalanced delimiter warnings** - Show error hint when `$` or `$$` delimiters are unbalanced (detected by `detectUnrenderedLatex()`)
- [ ] **Unrendered LaTeX command detection** - Show warning when LaTeX commands (e.g., `\frac`, `\sum`) appear outside of `$...$` delimiters
- [ ] **Integrate with auto-retry mechanism** - Use detected errors to provide context-aware retry hints (via `getRetryHint()` in prompts.rs)

## P1: New Tools

- [ ] **Switch main Agent logic to multimodal format** - Once `to_multimodal_messages` tests are passing (from Jules PR), integrate it into the main chat loop to support native images via OpenRouter/OpenAI.
- Code Tool: Run Python Code in Sandbox (one option: WASI via Wasmtime plus a small Rust mediator in a Tauri app?)
- YouTube Tool: Get Transcript & Summarize

## P1: Improve Tool UX

- [ ] Improve weather, stock, and web_search tool output UX. weather should get a full forecast as text for model (show as diagram in UI), stock should give price percentage changes and price history as text for model (show as graph in UI), web_search should show the full results as links and summary for model (show as list of websites visited in UI).

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

## P2: Multi-Provider Support

- [ ] Model management system that checks for free models from OpenRouter and updates the model list.
- [ ] Add support for other providers (e.g., Ollama, Anthropic).

## P2: Distribution & CI/CD

- [ ] Set up GitHub Actions for cross-platform builds
  - macOS: Uses existing `build-macos.sh` script
  - Auto-create releases with `.dmg`, `.msi`, `.AppImage`

## P2: Future Horizons (Documentation & Stubs)

### 1. Full Browser Control

- [ ] Investigate Playwright/Puppeteer Rust bindings for headful browsing.
- [ ] Implement a real DOM-interaction agent loop (click, type, scroll).
- [ ] Add visual reasoning (screenshots to VLM) to handle complex web apps.

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
