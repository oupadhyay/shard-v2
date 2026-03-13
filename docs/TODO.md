# TODO

## P0: Code Review Issues (Feb 2026)

- [ ] **Manual config merging** - (Deferred) Using `#[serde(default)]` patterns, full refactor to `figment` deemed too large for this pass.
- [ ] **Monolithic handleInput** ([main.ts:69-224](../src/main.ts)) - Split into `preparePayload`, `sendChatMessage`, etc.

## P1: Password Prompts

- [ ] Still requires 2 passwords prompts (there is always allow but 2 shouldn't be necessary?)

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
