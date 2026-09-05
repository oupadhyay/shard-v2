# Split Ownership Notes

The planned tool/API and provider extraction is complete. The Git dependency
cutover merged in [PR #123](https://github.com/oupadhyay/shard-v2/pull/123),
followed by the independent Clippy/benchmark repairs in
[PR #124](https://github.com/oupadhyay/shard-v2/pull/124).

## Consumed revisions

The initial cutover pins are below. [`Cargo.toml`](../src-tauri/Cargo.toml) and
[`Cargo.lock`](../src-tauri/Cargo.lock) are authoritative for subsequent updates.

| Repository | Initial consumed revision |
| --- | --- |
| shard-tool-api | [aea826a9](https://github.com/oupadhyay/shard-tool-api/commit/aea826a9e64b3035843aa8800f2f6c0f5fbe8b9a) |
| shard-external-tools | [b0fc4557](https://github.com/oupadhyay/shard-external-tools/commit/b0fc45572caf17af2e8f46fb3f0a181f084ef9dd) |
| shard-provider | [fb466f45](https://github.com/oupadhyay/shard-provider/commit/fb466f4528ad879ef8d2aceae50501b0bcf023fb) |

Distribution is through immutable GitHub revisions, not crates.io. A newer
standalone repository HEAD is not automatically consumed by the host.

## Dependency direction

The host is now the only workspace member; the proof-stage crate copies have
been removed. Dependencies remain one-way:

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
| Tool schema DTOs (`ToolDefinition`, `FunctionDefinition`, invocation shape) | `shard-tool-api` | No | Canonical DTOs live in the standalone crate. The host compatibility module contains re-exports only. Host, providers, MCP, and external tool executors share these types without depending on `Agent` or Tauri state. |
| Tool registry metadata | Split | Partially | Portable external-tool schemas/catalog entries can move with their implementations. The composed Shard registry stays host-owned because it combines availability, persona filtering, cache TTL, parallelism, and heartbeat draft-gating policy. |
| External API tools (`web_search`, `open_url`, weather, finance, Wikipedia, arXiv) | `shard-external-tools` | No | These compile in the host-free standalone crate. Shard owns the config adapter, hooks, cache, UI events, and deciding which tools are available. |
| YouTube transcript fetch/render | External tools crate, summarization host-side | No | Transcript retrieval/rendering now lives in `shard-external-tools` behind explicit process inputs. LLM summarization of very large transcripts stays in host/provider orchestration. |
| Memory/persona/action/self-edit/crystallization tools | Host repo | Yes | These mutate product state, sessions, memories, personas, or app files. They should use neutral schemas but not move into external API tooling. |
| Heartbeat draft gating and safe-tool policy | Host repo | Yes | This is product workflow/approval logic, not tool implementation. |

## Gemini Files, embeddings, and vision fallback

| Area | Owner after split | Keep here? | Notes |
| --- | --- | --- | --- |
| Gemini Files API transport (`upload`, `delete`) | `shard-provider` | No | Resumable upload/delete protocol and DTOs are extracted. The host owns when to upload images, which chat messages reference file URIs, and cleanup lifecycle. |
| Embedding API transport | `shard-provider` | No | Gemini embedding request/response code is extracted. The memory system owns chunking, cache invalidation, vector schema, when to embed, and search. |
| Gemini chat wire types, request shaping, streaming transport | `shard-provider` | No | Shard retains model/key/endpoint lookup, retry/fallback, state, tool execution and UI events. |
| OpenAI-compatible vision transport | `shard-provider` | No | Request shaping, HTTP transport and response extraction take explicit caller inputs. |
| Vision fallback policy | Host repo | Yes | Deciding when a non-vision chat model needs image-to-text preprocessing, model priority, and prompt context stays in Shard. |

Rule of thumb: provider/tool crates may perform stateless protocol work from
explicit inputs; the Shard host owns durable state, workflow policy, UI events,
approval gates, endpoint/key lookup, and model-selection decisions.

## Compatibility and remaining scope

`tool_api.rs`, `external_tools.rs`, `llm_provider.rs`, `gemini_files.rs`,
`gemini_embedding.rs`, and `agent/gemini/mod.rs` retain live host call sites.
They re-export canonical crate APIs (including the `AgentEvent` compatibility
alias), not copies of portable implementations. Keep them until callers no
longer need them; removing them now would only create import churn.

OpenAI-compatible **chat** in `agent/openrouter.rs` remains host-side. It was
not included in the Gemini chat / OpenAI-compatible **vision** extraction, and
is not a second copy of a chat transport in `shard-provider`. Any future move
is a separate functional boundary requiring its own wire tests and GUI checks.

## Future changes and verification

1. Change and validate the owning standalone repository first.
2. Merge it, then update Shard's `rev` to that exact reviewed commit.
3. For tool-contract changes, coordinate all three consumers on one tool-api
   revision before updating the host lockfile.
4. Run `cargo tree --locked -i shard-tool-api` and `cargo tree --locked -d`;
   there must be one tool-api source and no sibling dependency cycle.
5. Run host checks and the affected native GUI scenarios in
   [AGENTS.md](../AGENTS.md). Standalone unit tests run in their own repositories;
   host workspace tests do not run Git dependency unit tests.

Do not recreate in-tree crate copies. Host GUI testing must preserve hooks,
cache, persistence, YouTube short/long rendering and heartbeat restrictions,
Gemini streaming/tool calls, Files lifecycle, memory search, and vision fallback.
Linux Tauri/WebKit coverage does not validate macOS-only native behavior.
