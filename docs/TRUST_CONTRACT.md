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
by that tool. Crystallization by sketch ID generates a persona after approval;
it is approval of that operation, not a preview of the final generated prose.
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
