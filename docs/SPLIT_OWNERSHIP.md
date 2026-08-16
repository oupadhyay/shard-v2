# Split Ownership Notes

This repo is being prepared for future crate/repository splits without moving
code prematurely. The goal is to make ownership explicit at API boundaries first.

## Dependency direction

The in-repository workspace is the proof stage before code moves to separate
repositories. Dependencies must remain one-way:

```text
shard host ────────────────> shard-tool-api
     ├─────────────────────> shard-external-tools ──> shard-tool-api
     └─────────────────────> shard-provider ─────────> shard-tool-api
```

`shard-provider` and `shard-external-tools` must not depend on each other. The
Shard host is the only layer that composes provider transport, external tools,
durable state, UI events, and workflow policy.

## Tooling

| Area | Owner after split | Keep here? | Notes |
| --- | --- | --- | --- |
| Tool schema DTOs (`ToolDefinition`, `FunctionDefinition`, invocation shape) | `shard-tool-api` | No | The canonical DTOs live in the workspace crate. The host compatibility module contains re-exports only. Host, providers, MCP, and external tool executors share these types without depending on `Agent` or Tauri state. |
| Tool registry metadata | Split | Partially | Portable external-tool schemas/catalog entries can move with their implementations. The composed Shard registry stays host-owned because it combines availability, persona filtering, cache TTL, parallelism, and heartbeat draft-gating policy. |
| External API tools (`web_search`, `open_url`, weather, finance, Wikipedia, arXiv) | `shard-external-tools` | No | These now compile in the host-free workspace crate. Shard owns the config adapter, hooks, cache, UI events, and deciding which tools are available. |
| YouTube transcript fetch/render | External tools crate, summarization host-side | No | Transcript retrieval/rendering now lives in `shard-external-tools` behind explicit process inputs. LLM summarization of very large transcripts stays in host/provider orchestration. |
| Memory/persona/action/self-edit/crystallization tools | Host repo | Yes | These mutate product state, sessions, memories, personas, or app files. They should use neutral schemas but not move into external API tooling. |
| Heartbeat draft gating and safe-tool policy | Host repo | Yes | This is product workflow/approval logic, not tool implementation. |

## Gemini Files, embeddings, and vision fallback

| Area | Owner after split | Keep here? | Notes |
| --- | --- | --- | --- |
| Gemini Files API transport (`upload`, `delete`) | Gemini/provider API crate | Eventually no | The resumable upload/delete protocol is provider-specific API code. The host still owns when to upload images, which chat messages reference file URIs, and cleanup lifecycle. |
| Embedding API transport | Gemini/provider API crate | Eventually no | Gemini embedding request/response code is provider transport. The 5-tier memory system owns chunking, cache invalidation, vector schema, when to embed, and how embeddings are searched. |
| Vision fallback policy | Host repo | Yes | Deciding when a non-vision chat model needs image-to-text preprocessing, model priority, and prompt context is host orchestration. The OpenAI-compatible vision request builder can later share provider transport helpers, but the fallback workflow should not move into the provider crate. |

Rule of thumb: provider/tool crates may perform stateless protocol work from
explicit inputs; the Shard host owns durable state, workflow policy, UI events,
approval gates, endpoint/key lookup, and model-selection decisions.
