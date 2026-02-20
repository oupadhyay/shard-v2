# TODO

## P0: Code Review Issues (Feb 2026)

### Medium

- [ ] **Manual config merging** - (Deferred) Using `#[serde(default)]` patterns, full refactor to `figment` deemed too large for this pass.
- [x] **Global frontend state** ([main.ts:42-48](../src/main.ts)) - Encapsulated in `ChatState` class (`src/state.ts`)
- [ ] **Monolithic handleInput** ([main.ts:69-224](../src/main.ts)) - Split into `preparePayload`, `sendChatMessage`, etc.

---

## P0: Clawdbot Memory Learnings

### 1. Compaction + Pre-Compaction Memory Flush

- [x] Add `context_size` config per model (e.g., 131K for GPT-OSS, 1M for all Gemini models)
- [x] Track token usage in conversation history
- [x] Trigger compaction when approaching ~50% of context window
- [x] Pre-compaction flush: silent LLM turn to save important facts to `memory/YYYY-MM-DD.md` before summarization
- [x] Store compaction summaries in session transcripts (JSONL)

### 2. Chunking Pipeline for Topics/Insights

- [x] Chunk content into ~400 tokens with 80-token overlap
- [x] Store chunks with line range metadata: `{chunk_id, text, start_line, end_line, embedding}`
- [x] Update `find_relevant_context()` to search chunks instead of whole documents
- [x] Return specific snippets instead of entire topic files

### 3. sqlite-vec for Embedding Storage

- [x] Replace JSON index files with SQLite database (`memories.sqlite`)
- [x] Use `sqlite-vec` extension for vector similarity search
- [x] Use FTS5 for BM25 keyword matching (hybrid search in one DB)
- [x] Add `embedding_cache` table to avoid re-embedding unchanged content

### 4. Session Memory Hooks + Descriptive Slugs

- [ ] On conversation clear/new session: extract last N messages
- [ ] Generate descriptive slug via LLM (e.g., "api-design-discussion")
- [ ] Save to `memory/YYYY-MM-DD-\<slug\>.md` for searchable session transcripts

### 5. Explicit Memory Search Tools

- [ ] Add `memory_search` tool: semantic search across all memory tiers
  - Params: `query`, `max_results`, `min_score`
  - Returns: `{path, start_line, end_line, score, snippet, source}`
- [ ] Add `memory_get` tool: read specific lines from a memory file
  - Params: `path`, `from`, `lines`
- [ ] Keep existing silent RAG injection for baseline context

## P0: Read Page via Browser Tool

- [ ] Add `open_url` tool to allow Shard to read any URL (HTML/browser/DOM?)

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
