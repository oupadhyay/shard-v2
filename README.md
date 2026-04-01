# Shard AI Assistant

Shard is a high-performance, privacy-focused AI assistant built with **Tauri 2**, **Rust**, and **TypeScript**. It runs as a native desktop app with a transparent, glassy overlay UI — summoned instantly via a global shortcut — and leverages multiple LLM providers with autonomous tool calling, persistent memory, and on-demand persona loading.

## ✨ Key Features

- **Multi-Provider Chat** — Gemini 2.5/3 Flash, GPT-OSS 120B (OpenRouter / Groq / Cerebras), LLaMA 3.3, Stepfun 3.5 Flash. Automatic fallback across providers.
- **Autonomous Tools** — Weather, Wikipedia, Stocks, ArXiv (HTML parsing), Web Search (Brave), URL reader. Results are cached with per-tool TTLs.
- **Multimodal Intelligence** — Screenshot-to-chat via `Ctrl+Space`. Gemini models use the native Files API; non-vision models route through a Vision LLM pipeline (Molmo, Nemotron, Qwen 2.5 VL).
- **Deep Research Mode** — Multi-step synthesis from ArXiv, Wikipedia, and web sources for complex queries.
- **Personas Engine** — Load / unload specialized personas and instruction sets on-demand via `load_persona` / `unload_persona` tools. Personas are `.md` files with optional YAML frontmatter for `required_tools`.
- **Persistent Sessions** — SQLite-backed session management with LLM-generated titles, summaries, and full message history. Switch between conversations via the Sessions modal.
- **5-Tier Memory** — Core facts → Topic summaries → Atomic insights → Session transcripts → Interaction JSONL, all backed by SQLite with sqlite-vec embeddings and FTS5 keyword search.
- **Background Jobs** — Automated 6-hour cycles for memory summarization, cleanup, insight extraction, and up-leveling — powered by free-tier background LLMs.
- **Auto-Retry** — Automatic retry on empty responses and KaTeX rendering errors, with context-aware hints injected for the model.
- **Privacy First** — API keys stored in the OS keychain (macOS Keychain / Windows Credential Manager / Linux Secret Service). No middleman servers.

## 🏗️ Architecture Overview

| Layer        | Stack                                            | Key Files                                                                |
|--------------|--------------------------------------------------|--------------------------------------------------------------------------|
| **Frontend** | TypeScript + Vite + Vanilla CSS                  | `src/main.ts`, `src/state.ts`, `src/ui/`, `src/styles.css`               |
| **Backend**  | Rust + Tauri 2 + SQLite + sqlite-vec             | `src-tauri/src/` (20+ modules)                                           |
| **IPC**      | Tauri commands + event emitter                   | `#[tauri::command]` ↔ `invoke<T>()`                                      |
| **Data**     | `~/Library/Application Support/dev.ojasw.shard/` | `memories.sqlite`, `tool_cache.json`, `personas/`, `last_run.json`         |

See [`AGENTS.md`](./AGENTS.md) for the full module-level architecture reference.

## 🚀 Getting Started

### Prerequisites

- **Node.js** ≥ 18 and **npm**
- **Rust** (stable) with `cargo`
- **Tauri 2 CLI**: `cargo install tauri-cli --version "^2"`

### Development

```bash
npm install
npm run tauri dev        # Launch with hot-reload (Vite on :1420)
```

### Testing

```bash
# Frontend (vitest + jsdom)
npm test

# Backend (Rust unit tests)
cd src-tauri && cargo test

# Benchmarks (Criterion)
cd src-tauri && cargo bench
```

### Production Build

```bash
npm run build:macos      # macOS .dmg
```

## 📝 Configuration

Shard is configured entirely through the in-app Settings modal:

| Setting              | Description                                                       |
|----------------------|-------------------------------------------------------------------|
| **API Keys**         | Gemini, OpenRouter, Brave, Groq, Cerebras (stored in OS keychain) |
| **Chat Model**       | Select from available chat models per provider                    |
| **Background Model** | LLM used for automated memory jobs (Groq/Cerebras/OpenRouter)     |
| **Vision Model**     | LLM used for image understanding on non-vision chat models        |
| **System Prompt**    | Custom personality / instruction override                         |
| **Enable Tools**     | Toggle autonomous tool calling                                    |
| **Research Mode**    | Multi-step deep research with web synthesis                       |

## 📚 Documentation

| Document                                           | Purpose                                                              |
|----------------------------------------------------|----------------------------------------------------------------------|
| [`AGENTS.md`](./AGENTS.md)                         | Full architecture reference (modules, tools, memory, retrieval, CSS) |
| [`docs/BENCH.md`](./docs/BENCH.md)                 | Benchmark suite descriptions and run commands                        |
| [`docs/BENCH_RESULTS.md`](./docs/BENCH_RESULTS.md) | Historical benchmark measurements                                    |
| [`docs/RETRIEVAL.md`](./docs/RETRIEVAL.md)         | Hybrid search strategy (sqlite-vec + FTS5)                           |
| [`docs/TODO.md`](./docs/TODO.md)                   | Roadmap and planned features                                         |
| [`lessons_learned.md`](./lessons_learned.md)       | Engineering lessons from development                                 |

---

Built by Ojasw with ❤️ using [Tauri](https://tauri.app/).
