/**
 * Feature parity tests verifying that dedicated.ts imports and events
 * match the ambient (main.ts) window capabilities.
 *
 * These are static analysis tests — they parse the source files and verify
 * that the dedicated window handles all the same events and uses the same
 * shared functions as the ambient window.
 */
import { describe, it, expect } from 'vitest';
import fs from 'fs';
import path from 'path';
import { fileURLToPath } from 'url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));

const dedicatedSrc = fs.readFileSync(
  path.resolve(__dirname, '../dedicated.ts'),
  'utf-8'
);

const mainSrc = fs.readFileSync(
  path.resolve(__dirname, '../main.ts'),
  'utf-8'
);

describe('Dedicated window event listener parity', () => {
  // All Tauri event names that the ambient window listens to
  const requiredEvents = [
    'AGENT_RESPONSE_CHUNK',
    'AGENT_REASONING_CHUNK',
    'AGENT_TOOL_CALL',
    'AGENT_TOOL_RESULT',
    'AGENT_RETRY',
    'AGENT_RETRY_EXHAUSTED',
    'AGENT_ERROR',
    'AGENT_FALLBACK',
    'AGENT_CRON_STARTED',
    'SESSIONS_UPDATED',
    'TRIGGER_OCR',
    'PROACTIVE_MESSAGE',
  ];

  // Events that are intentionally ambient-only
  const ambientOnlyEvents = [
    'START_HIDE',
    'START_SHOW',
    'SCREEN_CONTEXT_READY',
    'AGENT_PROCESSING_START',  // no-op in ambient too
  ];

  for (const event of requiredEvents) {
    it(`listens for EVENTS.${event}`, () => {
      expect(dedicatedSrc).toContain(`EVENTS.${event}`);
    });
  }

  for (const event of ambientOnlyEvents) {
    it(`intentionally omits ambient-only EVENTS.${event}`, () => {
      // These should NOT be in dedicated.ts (they're ambient-specific)
      // AGENT_PROCESSING_START is a no-op, others are window-visibility related
      expect(dedicatedSrc).not.toContain(`EVENTS.${event}`);
    });
  }
});

describe('Dedicated window shared function usage', () => {
  const sharedFunctions = [
    'createStreamingAssistantMessage',
    'renderStreamingContent',
    'shouldSkipStreamingChunk',
    'createThinkingElement',
    'updateThinkingElement',
    'createToolCallElement',
    'updateToolResult',
    'addMessage',
    'addProactiveMessage',
    'getOrCreateWebSearchContainer',
    'resetWebSearchContainer',
    'isWebSearchTool',
    'createWebSearchQueryElement',
    'updateWebSearchCount',
    'clearKatexErrors',
    'getKatexErrors',
    'detectUnrenderedLatex',
    'populateHeartbeatsPanel',
  ];

  for (const fn of sharedFunctions) {
    it(`imports and uses ${fn}`, () => {
      expect(dedicatedSrc).toContain(fn);
    });
  }
});

describe('Dedicated window feature parity', () => {
  it('handles clipboard image paste', () => {
    expect(dedicatedSrc).toContain("addEventListener(\"paste\"");
  });

  it('handles backspace to remove last image', () => {
    expect(dedicatedSrc).toContain("Backspace");
    expect(dedicatedSrc).toContain("attachedImages.pop()");
  });

  it('has focus tracking for blur-state CSS', () => {
    expect(dedicatedSrc).toContain("app-focused");
    expect(dedicatedSrc).toContain("app-unfocused");
  });

  it('rewinds history on error retry', () => {
    // The error retry handler should call rewind_history
    expect(dedicatedSrc).toContain("rewind_history");
  });

  it('uses resizeImage for clipboard paste OCR', () => {
    expect(dedicatedSrc).toContain("resizeImage");
  });

  it('includes heartbeat cooldown in settings', () => {
    expect(dedicatedSrc).toContain("heartbeat-cooldown");
    expect(dedicatedSrc).toContain("heartbeat_global_cooldown_secs");
  });

  it('calls populateHeartbeatsPanel in settings', () => {
    expect(dedicatedSrc).toContain("populateHeartbeatsPanel(settingsModal)");
  });

  it('loads proactive messages on init', () => {
    expect(dedicatedSrc).toContain("loadProactiveMessages");
  });

  it('handles provider fallback with dedup flag', () => {
    expect(dedicatedSrc).toContain("fallbackShownThisTurn");
  });

  it('resets streaming state after chat turn', () => {
    // Both windows call resetForNewTurn() which clears fallbackShownThisTurn
    expect(dedicatedSrc).toContain("resetForNewTurn");
  });
});

describe('Streaming handler parity between main.ts and dedicated.ts', () => {
  it('both use createStreamingAssistantMessage for new messages', () => {
    expect(mainSrc).toContain('createStreamingAssistantMessage()');
    expect(dedicatedSrc).toContain('createStreamingAssistantMessage()');
  });

  it('both use renderStreamingContent for HTML rendering', () => {
    expect(mainSrc).toContain('renderStreamingContent(');
    expect(dedicatedSrc).toContain('renderStreamingContent(');
  });

  it('both use shouldSkipStreamingChunk for whitespace guard', () => {
    expect(mainSrc).toContain('shouldSkipStreamingChunk(');
    expect(dedicatedSrc).toContain('shouldSkipStreamingChunk(');
  });

  it('neither window has inline <think> tag parsing (extracted to shared)', () => {
    // The literal "<think>" should only appear in renderStreamingContent
    // (which is in messages.ts), not duplicated in the window files
    const mainChunkHandler = mainSrc.split('AGENT_RESPONSE_CHUNK')[1]?.split('AGENT_REASONING_CHUNK')[0] || '';
    const dedicatedChunkHandler = dedicatedSrc.split('AGENT_RESPONSE_CHUNK')[1]?.split('AGENT_REASONING_CHUNK')[0] || '';

    expect(mainChunkHandler).not.toContain('<think>');
    expect(dedicatedChunkHandler).not.toContain('<think>');
  });
});
