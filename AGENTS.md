# shard-v2 Repository Guidance

## Purpose

`shard-v2` is the Shard desktop product: a Tauri/Rust host with a TypeScript
frontend. It composes providers, tools, durable application state, and native
UI behavior into the user-facing app.

Sibling repositories:

- [`shard-tool-api`](https://github.com/oupadhyay/shard-tool-api) — canonical,
  provider-neutral tool contract DTOs.
- [`shard-external-tools`](https://github.com/oupadhyay/shard-external-tools) —
  host-free external API and YouTube tool implementations.
- [`shard-provider`](https://github.com/oupadhyay/shard-provider) — host-free
  provider wire contracts and transports.

## Ownership

This repository owns:

- the ambient and dedicated frontends, Tauri commands, native-window behavior,
  and UI events;
- provider/model selection, endpoint and credential lookup, retry/fallback
  orchestration, and prompt/workflow composition;
- sessions, SQLite/vector persistence, memory chunking/invalidation/search,
  personas, MCP, actions, self-editing, and background jobs;
- tool registry composition, hooks, caching, availability, result persistence,
  and heartbeat approval/prohibition policy;
- image and Gemini Files lifecycle, long-transcript summarization, and the
  decision to invoke vision fallback.

Extracted Gemini transports, OpenAI-compatible vision transport, portable
external-tool execution, and canonical tool-contract DTOs live in the sibling
repositories. The existing OpenAI-compatible chat path remains in the host;
it was not part of this extraction. Compatibility re-exports remain for live
callers, but no proof-stage crate copies remain.

## Dependency Rules

Dependencies are one-way:

```text
shard-v2 ──> shard-tool-api
    ├──────> shard-external-tools ──> shard-tool-api
    └──────> shard-provider ─────────> shard-tool-api
```

- `shard-provider` and `shard-external-tools` must never depend on each other.
- Split crates must not depend on Tauri, persistence/SQLite, host configuration
  or key lookup, UI events, model selection, retry/fallback policy, or workflow
  approval policy.
- Do not create a second source of `shard-tool-api` in one Cargo graph. Rust
  treats identical types from path and Git sources as different nominal types.
- The Git cutover is complete. Do not recreate `src-tauri/crates/` copies.
  Make portable changes in their standalone repository, then update the host
  to a reviewed immutable revision.

See [`docs/SPLIT_OWNERSHIP.md`](docs/SPLIT_OWNERSHIP.md) for the detailed
boundary map.

## Build and Validation

Install dependencies from the repository root:

```bash
npm ci
```

Frontend checks:

```bash
npm test -- --run
npm run build
```

Rust checks run from `src-tauri`:

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
```

Build the native application from the repository root with:

```bash
npm run tauri build
```

Use targeted checks while iterating, then scale to the affected boundary. Do
not hide unrelated baseline failures or manufacture passing results.

## Updating Split Revisions

Use immutable Git commit SHAs, not moving branches. When updating
`shard-tool-api`, update the host and every consuming split crate together so
Cargo resolves one nominal type source. Then regenerate `src-tauri/Cargo.lock`
and audit the graph:

```bash
cd src-tauri
cargo update -p shard-tool-api
cargo update -p shard-external-tools
cargo update -p shard-provider
cargo tree -i shard-tool-api
cargo tree -d
```

First merge and validate the standalone repository change, then test
`shard-v2` against that exact revision. `src-tauri/Cargo.toml` and its lockfile
are authoritative for consumed revisions; see `docs/SPLIT_OWNERSHIP.md` for
the initial cutover pins. Standalone crate tests run in their own repositories:
host `cargo test --workspace` does not run Git dependency unit tests, and
`cargo test -p` cannot run dependency tests requiring dev-dependencies.

## Native GUI Regression Matrix

Run the real Tauri application (`npm run tauri dev`), not only the Vite page.
At minimum check normal chat streaming, persistence, cancellation, tool-call
events/output, and one external tool. Add the relevant scenarios below whenever
their boundary changes:

- external tools: web/open-URL behavior; YouTube short rendering and long host
  summarization; heartbeat prohibition; one non-YouTube dispatch;
- provider chat: normal Gemini streaming, reasoning/signature handling, tool
  calls, retries, and host UI events;
- Gemini Files: image upload, use in chat history, and cleanup;
- embeddings: memory creation, embedding, retrieval, and search;
- vision: image input and OpenAI-compatible fallback while retaining host model
  priority and prompt composition;
- frontend/native shell: both ambient and dedicated views, settings, sessions,
  and representative empty/loading/error states.

Linux Orb GUI checks exercise Tauri/WebKit and native IPC, but cannot validate
macOS-only NSPanel, private API, shortcut, or vibrancy behavior. Validate those
changes on macOS as well.
