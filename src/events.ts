/**
 * Event names for Tauri's event system
 */
export const EVENTS = {
  AGENT_RETRY: "agent-retry",
  AGENT_RETRY_EXHAUSTED: "agent-retry-exhausted",
  AGENT_RESPONSE_CHUNK: "agent-response-chunk",
  AGENT_REASONING_CHUNK: "agent-reasoning-chunk",
  AGENT_TOOL_CALL: "agent-tool-call",
  AGENT_TOOL_RESULT: "agent-tool-result",
  AGENT_ERROR: "agent-error",
  AGENT_FALLBACK: "agent-fallback",
  AGENT_PROCESSING_START: "agent-processing-start",
  AGENT_CRON_STARTED: "agent-cron-started",
  SCREEN_CONTEXT_READY: "screen-context-ready",
  SESSIONS_UPDATED: "sessions-updated",
  TRIGGER_OCR: "trigger-ocr",
  START_HIDE: "start-hide",
  START_SHOW: "start-show",
  PROACTIVE_MESSAGE: "proactive-message",
  /**
   * Fired when the agent edits an allow-listed self-file via `edit_file`.
   * Payload: `EditOutcome` from `self_files.rs` —
   *   { path, abs_path, before, after, unified_diff, replacements }
   * Used by the upcoming file-tree + diff-viewer UI (see TODO).
   */
  FILE_EDITED: "file-edited",
} as const;
