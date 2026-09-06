# Host trust contract

## Self-changes require review

The host registry's `draft_gated` flag applies to interactive chat **and**
autonomous heartbeat dispatch. It covers `edit_file` (configuration, personas,
heartbeat specifications), heartbeat CRUD, `rollback_self_edit`, and
`crystallize_sketch`. A model tool call is not proof of user consent, even when
it follows a user message. These tools queue a concrete draft; only the native
approval command executes it. This deliberately adds confirmation for explicit
chat requests too. Direct user-operated settings remain direct actions.

MCP cannot grant Shard approval and refuses self-file edits, including its old
handler API. Read tools and existing memory/action tools retain their existing
behavior. This is not a general permissions framework for every external tool.
The Python runtime has only its scratch directory preopened, not host settings.
Short wake-up timers retain their existing behavior; their eventual background
self-changes still pass through approval.

Approval authorizes the operation and arguments shown, not every future change
by that tool. Crystallization generates persona text before asking for approval.
The stored proposal contains the full text and target file. Approval saves that
text without another model call. Older sketch-only proposals fail without saving;
the user must request a new draft with text to review.
Rollback without an event ID means the latest restorable event at execution.
Allow-list and syntax validation still apply where the existing executor uses
them; validation alone is never approval.

## Decision is not execution

`proactive_queue.reviewed_at` and `approved` retain their existing storage shape.
A separate additive `proactive_execution` table holds `status` and `result`.
The frontend DTO adds nullable `execution_status` and `execution_result`.

* Pending: no saved decision and no execution.
* Rejected: explicit `approved = false`; no execution.
* Approved, unknown: approval and the execution claim commit in one SQLite
  transaction **before** effects. Unknown means in flight **or** interrupted.
* Approved, succeeded: the executor returned success and its result was saved.
* Approved, failed: validation or execution returned an error, saved separately
  from approval. Partial effects remain possible.

Only one pending-to-approved transition can win. Reject/dismiss cannot overwrite
a claim. Failed claim persistence rolls the transaction back without executing.
If result persistence fails after an effect, the durable state remains unknown.
Neither failed nor unknown actions are automatically retried. Inspect actual
effects before creating another action. A failed IPC response is not evidence
that execution did not occur; the UI reads the saved decision before allowing a
retry, and fails closed when status cannot be read.

Initialization backfills historical claimed/approved drafts with unknown, not
invented success or rejection. Historical null approval remains null. No history
is deleted. Execution results are authoritative in the queue even if the extra
session-history message fails to save. Reviewed execution records remain
available through proactive-message reads; badge counts still count pending
review only. There is no exactly-once guarantee across SQLite and filesystem or
network effects, and no automatic recovery replay.

## Unfinished work stays visible

Both windows show a cross-session **Needs attention** section for failed and
unknown actions and unfinished saved plans. It refreshes when chat loads, a turn
ends, an action is reviewed, a proposal arrives, or the window gains focus. These
items are read-only: opening this section never retries an action. A failed read
keeps the last loaded items and shows an error. At most 100 failed/unknown actions
are shown at once. Completed and cancelled plan steps need no follow-up; blocked
steps do. A completed parent does not hide unfinished steps.

This view covers stored action plans, not every promise in conversation prose.
It does not infer that a plan succeeded from the assistant's wording.

## Reduced automatic memory, not private chat

The compatible stored key is still `incognito_mode`; the user-facing label is
**Reduce Automatic Memory**. It skips automatic chat RAG/peer-memory retrieval,
pending-action context injection, compaction, new interaction memory logging,
automatic transcript archival, and screen context while enabled. Clearing chat
does not bypass the archival guard.

Conversation history (including attachments) is still saved locally and used
as conversation context sent to the selected provider. Explicit memory tools
can still read stored memories. Existing save/update/refresh-memory guards stay
in place. Scheduled tasks, background crystallization, and MCP are separate
operations, not stopped by this control. Work already in flight is not revoked.
Turning memory back on can learn from retained history; this is not per-message
privacy provenance. No historical data is deleted or retention policy changed.

Genuinely private sessions would require a separate product decision about
history/attachment persistence, provider behavior, explicit tools, background
work, and transitions between modes. This control makes none of those promises.

## Review and native checks (September 2026)

* `cargo test --workspace -- --test-threads=1`: 544 tests passed. The normal
  parallel run hit the existing endpoint-override race in
  `agent_provider_tests::gemini_turn::http_failure_emits_error_and_returns_err`.
  The endpoint unit tests use a separate lock from the agent test harness.
* `cargo check --workspace --all-targets`: passed.
* `npm test -- --run`: 175 tests passed. `npm run build`: passed.
* Native Linux Tauri/WebKit ran under Xvfb and Openbox with a separate temporary
  HOME and XDG directories. Seeded SQLite proposals were exercised through the
  actual app, not mocked IPC: mouse approval saved the exact persona text;
  rejection wrote no file; a sketch-only legacy proposal failed without model
  generation; repeat approval was refused. Failed/unknown states and an unfinished
  plan remained visible in both windows and after app restart. Legacy unknown
  approval stayed null. The attention section had no replay buttons.
* Native screenshots of persona review, attention, and privacy settings were
  inspected. The dedicated chat area starts below the attention section; both
  views fit without horizontal overflow. Persona generation before approval is
  covered separately by a wiremock test that asserts exactly one provider request.
* Native startup exposed a CommonJS default-export mismatch in the Markdown
  plugin. The import now handles its wrapper; this was needed to load the actual
  app, not just pass tests. GUI checks used the production frontend build through
  Vite preview with `tauri dev`.

No live provider keys were configured in the isolated GUI profile. Live chat,
live external tools, and macOS NSPanel/vibrancy/Spaces behavior were not verified.
The frontend design-study branch is separate and was not merged into this work.
