# Shard AI Assistant

Shard is a high-performance, privacy-focused AI assistant built with **Tauri**, **Rust**, and **React**. It is designed to be a powerful, extensible, and aesthetically pleasing desktop application that leverages the latest advancements in LLMs.

## 🚀 Key Features

* **Multi-Model Support**: Gemini 2.0/2.5 Pro & Flash, Llama 3.3, GPT-OSS, Gemma 3 and Olmo 3.1.
* **Autonomous Tools**: Weather, Wikipedia, Stocks, ArXiv (enhanced HTML parsing), Web Search.
* **Multimodal Intelligence**: Take screenshots to chat about them. Shard can analyze and reason about visual content.
* **Deep Research**: Capable of conducting in-depth research by synthesizing information from multiple sources (ArXiv, Wikipedia, Web) to answer complex queries.
* **Privacy First**: Your API keys are stored locally. No middleman servers.

## 🔄 Feature: Background Jobs

Located in `src-tauri/src/background.rs`. Two LLM-powered jobs run **sequentially** every 6 hours:

| Job | Function |
|-----|----------|
| **Summary** | Analyzes last 12h of interactions, extracts topics, updates `memories/topics/*.md` |
| **Cleanup** | Identifies generic/redundant entries, removes them (fallback: date-based cleanup > 30 days) |

**Force-trigger via console:**

```javascript
await window.__TAURI__.core.invoke("force_summary")
await window.__TAURI__.core.invoke("force_cleanup")
await window.__TAURI__.core.invoke("rebuild_topic_index")  // Regenerate index after manual edits
```

## 🧠 Feature: 3-Tier Memory Architecture

| Tier | File(s) | Purpose | In Prompt | Retrieval |
|------|---------|---------|-----------|-----------|
| **1. Core** | `MEMORIES.json` | User preferences, critical facts | ✅ Always | Direct load |
| **2. Focused** | `topics/*.md` + `index.json` | Topic-specific summaries | ⚠️ On demand | RAG (Top-1) |
| **3. Interaction** | `interactions/*.jsonl` | Full conversation history | ❌ Never | RAG (Top-5) |

**Tier 1 - Core**: Always loaded into system prompt. Tool: `save_memory(category, content, importance)`

**Tier 2 - Focused**: Embedding-based retrieval. Tools: `read_topic_summary`, `update_topic_summary`

**Tier 3 - Interaction**: Daily JSONL files with embeddings. Cosine similarity search.

## 📝 Configuration

Shard allows extensive configuration via the UI:

* **API Keys**: Securely input your Gemini and OpenRouter keys.
* **Model Selection**: Choose your preferred model for each conversation.
* **System Prompt**: Customize the agent's personality and instructions.
* **Jailbreak Mode**: Toggle unrestricted modes for specific use cases.

---

Built by Ojasw with ❤️ using [Tauri](https://tauri.app/).
