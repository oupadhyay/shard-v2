/**
 * Event names for Tauri's event system
 */
export const EVENTS = {
  AGENT_RETRY: "agent-retry",
  AGENT_RESPONSE_CHUNK: "agent-response-chunk",
  AGENT_REASONING_CHUNK: "agent-reasoning-chunk",
  AGENT_TOOL_CALL: "agent-tool-call",
  AGENT_TOOL_RESULT: "agent-tool-result",
  AGENT_ERROR: "agent-error",
  AGENT_FALLBACK: "agent-fallback",
  AGENT_PROCESSING_START: "agent-processing-start",
  SCREEN_CONTEXT_READY: "screen-context-ready",
  TRIGGER_OCR: "trigger-ocr",
  START_HIDE: "start-hide",
  START_SHOW: "start-show",
} as const;
