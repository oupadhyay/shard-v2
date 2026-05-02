# Shard Eval Harness (Evaluator-as-Judge)

Whole-system behavioral evaluation for the Shard agent. **Not** a unit test or
benchmark — those already cover tokenization, schema serialization, BM25,
caching math, persona file parsing, etc. This harness is for things only a real
end-to-end run can measure: prompt routing, RAG-influenced answers, multi-turn
coherence, retry recovery, persona fidelity.

## Architecture

```diagram
╭───────────────────╮      ╭────────────────────╮      ╭────────────────╮
│ scenarios/*.yaml  │─────▶│ examples/eval.rs   │─────▶│ Gemini API     │
│ (scripted prompts │      │ (mock Tauri app +  │      │ (Gemma 4 31B,  │
│  + rubrics)       │      │  real Agent)       │      │  default)      │
╰───────────────────╯      ╰─────────┬──────────╯      ╰────────────────╯
                                     │ captures via Tauri events
                                     ▼
                           ╭───────────────────────╮
                           │ results/<timestamp>/  │
                           │   01_xxx.md           │
                           │   02_xxx.md           │
                           │   SUMMARY.md          │
                           ╰─────────┬─────────────╯
                                     │ paste into thread
                                     ▼
                           ╭───────────────────────╮
                           │ Claude (judge)        │
                           │ fills SUMMARY rubric  │
                           ╰───────────────────────╯
```

The harness drives `Agent::process_message` directly through a `tauri::test::mock_app`,
so every layer that runs in production also runs here: history, RAG, tool
dispatch, retry loop, compaction triggers. Events emitted by the agent
(`agent-response-chunk`, `agent-tool-call`, `agent-tool-result`, `agent-error`,
`agent-retry`) are captured by listeners on the mock app handle.

## Running

```bash
cd src-tauri

# minimum: Gemini key for the chat model
export GEMINI_API_KEY=...

# optional: Brave key so web_search actually returns hits
export BRAVE_API_KEY=...

# default model is gemma-4-31b-it via Gemini
cargo run --example eval --features eval

# or pick a different model
SHARD_EVAL_MODEL=gemini-2.5-flash cargo run --example eval --features eval

# or point at a different scenarios dir
SHARD_EVAL_SCENARIOS=eval/my-scenarios cargo run --example eval --features eval
```

Results are written to `eval/results/<UTC-timestamp>/`. Each scenario gets its
own `<id>.md` with transcript, objective check report, and a subjective rubric
copied verbatim from the YAML. `SUMMARY.md` is a single-page table for the
judge to fill in.

## Scenario YAML

```yaml
id: 04_some_unique_slug         # required, used for filename
name: Human-readable name        # required
description: >
  What aspect of behavior is being evaluated. Free text.
turns:                           # required, one entry per user turn
  - user: "First message"
  - user: "Follow-up message"
expectations:                    # optional, evaluated automatically
  must_call_tools: [web_search]  # tools that MUST be invoked
  must_not_call_tools: []        # tools that MUST NOT be invoked
  must_contain: ["Paris"]        # case-insensitive substring in ANY assistant turn
  must_not_contain: ["I don't know"]  # ditto
  no_errors: true                # default true; fails if agent-error fired
rubric:                          # optional, sent to Claude judge later
  - "Did the agent stay on topic across turns?"
  - "Is the answer factually correct?"
```

The objective checks decide the green/red marker in `SUMMARY.md`. The rubric is
purely for the human (or Claude) judge — keep it focused on things automation
can't measure: tone, concision, factuality of free-form claims, persona
fidelity.

## Cost-conscious judging workflow

1. Run the harness locally — costs ~one `process_message` per turn against
   Gemini's free tier (Gemma 4 31B is free).
2. Open the resulting directory in a Claude thread when you have quota.
3. Paste `SUMMARY.md` plus 1–2 scenario `.md` files at a time.
4. Claude fills in the rubric column and notes regressions.

The objective-check column is computed automatically with zero judge tokens, so
you only spend Claude tokens on the parts that genuinely need judgment.

## Adding more scenarios

Good targets that existing tests / benches don't cover:

| Domain                | What to script                                                |
|-----------------------|---------------------------------------------------------------|
| Persona fidelity      | `load_persona` → tone check → `unload_persona` → revert check |
| KaTeX retry           | Prompt eliciting math; check `retries` is non-empty           |
| Empty-response retry  | Force reasoning-only via tricky prompt                        |
| Compaction quality    | 30+ turn conversation; verify key fact survives summarization |
| RAG relevance         | Plant fact in turn 1; query it in turn 20 (post-compaction)   |
| Tool failure recovery | Run with bogus BRAVE_API_KEY; check graceful degradation      |
| Cron / heartbeat      | Use `is_cron=true` path (requires harness extension)          |

Persona scenarios additionally need persona Markdown files seeded into the
mock app's `personas/` dir before `Agent::new` runs — left as an extension
once you have a couple of personas worth testing.

## Caveats

- **Stateless across scenarios.** Each scenario gets a fresh mock app, so no
  scenario can depend on memory written by a prior scenario. Multi-step
  state must live inside a single scenario's `turns:` list.
- **No persona seeding yet.** Personas live in `app_data_dir/personas/`; mock
  app's data dir is a fresh temp dir each run, so no personas are available.
  Add a `seed_personas:` field + copy step to scenario loader when needed.
- **Model nondeterminism.** Run scenarios 3–5 times before declaring a
  regression. The objective checks are robust; the rubric is not.
- **Free-tier rate limits.** Gemma 4 31B free tier has minute-level RPM caps.
  If you hit `429`, the harness will emit `agent-error` for that turn and the
  scenario will fail objectively — re-run after a cooldown.
