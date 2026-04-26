/**
 * Tests for the ChatController class (src/chat.ts).
 *
 * Verifies the full input → chat → post-process lifecycle, including guard
 * logic, state transitions, and stop-button management.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { mockIPC, clearMocks } from '@tauri-apps/api/mocks';
import { ChatController, type ChatDOMRefs, type ChatHooks, type ChatIcons } from '../chat';
import { ChatState } from '../state';
import type { AttachedImage } from '../types';

// Mock window.crypto for UUID generation
Object.defineProperty(window, 'crypto', {
  value: {
    getRandomValues: (buffer: any) => {
      return require('crypto').randomFillSync(buffer);
    },
  },
});

// ── Helpers ───────────────────────────────────────────────────────────

function createDOM(): ChatDOMRefs {
  const chatArea = document.createElement('div') as HTMLDivElement;
  chatArea.id = 'chat-area';

  const inputField = document.createElement('textarea') as HTMLTextAreaElement;
  inputField.id = 'input-field';

  const stopBtn = document.createElement('button') as HTMLButtonElement;
  stopBtn.id = 'stop-btn';
  stopBtn.style.display = 'none';

  // Create image preview container
  const previewContainer = document.createElement('div');
  previewContainer.id = 'image-preview-container';
  document.body.appendChild(previewContainer);

  return { chatArea, inputField, stopBtn, imagePreviewContainerId: 'image-preview-container' };
}

const TEST_ICONS: ChatIcons = {
  stop: '<svg class="stop"></svg>',
  resend: '<svg class="resend"></svg>',
};

function makeImage(label = 'test'): AttachedImage {
  return {
    base64: `base64-${label}`,
    mimeType: 'image/png',
    ocrText: `ocr-${label}`,
  };
}

// ── Tests ─────────────────────────────────────────────────────────────

describe('ChatController', () => {
  let dom: ChatDOMRefs;
  let state: ChatState;
  let hooks: ChatHooks;
  let controller: ChatController;

  beforeEach(() => {
    clearMocks();
    vi.clearAllMocks();
    document.body.innerHTML = '';

    dom = createDOM();
    state = new ChatState();
    hooks = {
      onUserMessageRendered: vi.fn(),
      onTurnComplete: vi.fn(),
      onUpdateButtonStates: vi.fn(),
    };

    // Default IPC mock: chat resolves immediately
    mockIPC((cmd) => {
      if (cmd === 'chat') return null;
      if (cmd === 'retry_with_katex_hint') return null;
    });

    controller = new ChatController(dom, state, hooks, TEST_ICONS);
  });

  // ── Guards ────────────────────────────────────────────────────────

  describe('guards', () => {
    it('does nothing when input is empty and skipUi is false', async () => {
      dom.inputField.value = '';
      await controller.handleInput();

      expect(state.isProcessing).toBe(false);
      expect(hooks.onTurnComplete).not.toHaveBeenCalled();
    });

    it('does nothing when already processing', async () => {
      state.isProcessing = true;
      dom.inputField.value = 'Hello';
      await controller.handleInput();

      // onTurnComplete is only called inside the real flow
      expect(hooks.onTurnComplete).not.toHaveBeenCalled();
    });

    it('proceeds with skipUi=true even when input is empty', async () => {
      dom.inputField.value = '';
      state.lastUserMessage = 'resend this';
      await controller.handleInput(true);

      expect(hooks.onTurnComplete).toHaveBeenCalledTimes(1);
    });
  });

  // ── Input Capture ─────────────────────────────────────────────────

  describe('input capture', () => {
    it('clears the input field after sending', async () => {
      dom.inputField.value = 'Hello world';
      await controller.handleInput();

      expect(dom.inputField.value).toBe('');
    });

    it('stores last user message for resend', async () => {
      dom.inputField.value = 'Test message';
      await controller.handleInput();

      expect(state.lastUserMessage).toBe('Test message');
    });

    it('snapshots images and clears them from state', async () => {
      const img = makeImage();
      state.attachedImages = [img];
      dom.inputField.value = 'With image';
      await controller.handleInput();

      expect(state.lastAttachedImages).toHaveLength(1);
      expect(state.lastAttachedImages[0].base64).toBe('base64-test');
      expect(state.attachedImages).toHaveLength(0);
    });

    it('adds a user message to the chat area', async () => {
      dom.inputField.value = 'Hello';
      await controller.handleInput();

      const userMsg = dom.chatArea.querySelector('.message.user');
      expect(userMsg).not.toBeNull();
    });

    it('fires onUserMessageRendered hook', async () => {
      dom.inputField.value = 'Hello';
      await controller.handleInput();

      expect(hooks.onUserMessageRendered).toHaveBeenCalledTimes(1);
    });

    it('does NOT render user message when skipUi=true', async () => {
      state.lastUserMessage = 'resend';
      dom.inputField.value = 'resend';
      await controller.handleInput(true);

      const userMsg = dom.chatArea.querySelector('.message.user');
      expect(userMsg).toBeNull();
      expect(hooks.onUserMessageRendered).not.toHaveBeenCalled();
    });

    it('clears image preview container DOM', async () => {
      // Reuse the container created in beforeEach (createDOM appends it to body)
      const container = document.getElementById('image-preview-container')!;
      container.innerHTML = '<div class="image-preview">old</div>';

      dom.inputField.value = 'cleaner';
      await controller.handleInput();

      expect(container.innerHTML).toBe('');
    });
  });

  // ── Processing State ──────────────────────────────────────────────

  describe('processing state', () => {
    it('shows stop button during processing', async () => {
      let btnDisplay = '';
      mockIPC(async (cmd) => {
        if (cmd === 'chat') {
          // Capture state DURING the chat call
          btnDisplay = dom.stopBtn.style.display;
          return null;
        }
      });

      // Recreate controller with the new mock
      controller = new ChatController(dom, state, hooks, TEST_ICONS);

      dom.inputField.value = 'Hello';
      await controller.handleInput();

      expect(btnDisplay).toBe('inline-flex');
    });

    it('hides stop button after turn completes (not cancelled)', async () => {
      dom.inputField.value = 'Hello';
      await controller.handleInput();

      expect(dom.stopBtn.style.display).toBe('none');
      expect(dom.stopBtn.classList.contains('loading')).toBe(false);
    });

    it('leaves stop button visible after cancellation', async () => {
      mockIPC(async (cmd) => {
        if (cmd === 'chat') {
          // Simulate cancellation mid-turn
          state.isCancelled = true;
          return null;
        }
      });
      controller = new ChatController(dom, state, hooks, TEST_ICONS);

      dom.inputField.value = 'Hello';
      await controller.handleInput();

      // Button should NOT be hidden (allows resend)
      expect(dom.stopBtn.style.display).not.toBe('none');
    });

    it('sets stop button icon to stop icon during processing', async () => {
      let btnHTML = '';
      mockIPC(async (cmd) => {
        if (cmd === 'chat') {
          btnHTML = dom.stopBtn.innerHTML;
          return null;
        }
      });
      controller = new ChatController(dom, state, hooks, TEST_ICONS);

      dom.inputField.value = 'Hi';
      await controller.handleInput();

      expect(btnHTML).toBe(TEST_ICONS.stop);
    });

    it('resets isProcessing after turn, even on error', async () => {
      mockIPC((cmd) => {
        if (cmd === 'chat') throw new Error('network failure');
      });
      controller = new ChatController(dom, state, hooks, TEST_ICONS);

      dom.inputField.value = 'Hello';
      await controller.handleInput();

      expect(state.isProcessing).toBe(false);
      expect(hooks.onTurnComplete).toHaveBeenCalled();
    });
  });

  // ── State Reset ───────────────────────────────────────────────────

  describe('turn state reset', () => {
    it('clears fallbackShownThisTurn at start of turn', async () => {
      state.fallbackShownThisTurn = true;
      dom.inputField.value = 'Hi';
      await controller.handleInput();

      expect(state.fallbackShownThisTurn).toBe(false);
    });

    it('clears currentThinkingBlock at start of turn', async () => {
      state.currentThinkingBlock = document.createElement('div');
      dom.inputField.value = 'Hi';
      await controller.handleInput();

      // After resetForNewTurn, this should be null
      expect(state.currentThinkingBlock).toBe(null);
    });

    it('clears isCancelled at start of turn', async () => {
      state.isCancelled = true;
      dom.inputField.value = 'Hi';
      await controller.handleInput();

      // (It's reset in resetTurnState before any async work)
      expect(state.isCancelled).toBe(false);
    });
  });

  // ── Thinking Block Finalization ───────────────────────────────────

  describe('thinking block finalization', () => {
    it('finalizes open thinking blocks after turn', async () => {
      // Simulate an open thinking block in the chat area
      const thinkingEl = document.createElement('div');
      thinkingEl.className = 'message thinking-output';
      thinkingEl.setAttribute('data-complete', 'false');
      const summary = document.createElement('summary');
      summary.textContent = 'Thinking...';
      thinkingEl.appendChild(summary);
      dom.chatArea.appendChild(thinkingEl);

      dom.inputField.value = 'Hi';
      await controller.handleInput();

      expect(thinkingEl.getAttribute('data-complete')).toBe('true');
      expect(summary.textContent).toBe('Thought');
    });

    it('does not touch already-complete thinking blocks', async () => {
      const thinkingEl = document.createElement('div');
      thinkingEl.className = 'message thinking-output';
      thinkingEl.setAttribute('data-complete', 'true');
      const summary = document.createElement('summary');
      summary.textContent = 'Thought';
      thinkingEl.appendChild(summary);
      dom.chatArea.appendChild(thinkingEl);

      dom.inputField.value = 'Hi';
      await controller.handleInput();

      expect(summary.textContent).toBe('Thought');
    });
  });

  // ── Hooks ─────────────────────────────────────────────────────────

  describe('hooks', () => {
    it('calls onTurnComplete in finally block', async () => {
      dom.inputField.value = 'Hello';
      await controller.handleInput();

      expect(hooks.onTurnComplete).toHaveBeenCalledTimes(1);
    });

    it('calls onUpdateButtonStates in finally block', async () => {
      dom.inputField.value = 'Hello';
      await controller.handleInput();

      expect(hooks.onUpdateButtonStates).toHaveBeenCalledTimes(1);
    });

    it('calls hooks even when chat throws', async () => {
      mockIPC((cmd) => {
        if (cmd === 'chat') throw new Error('fail');
      });
      controller = new ChatController(dom, state, hooks, TEST_ICONS);

      dom.inputField.value = 'Hello';
      await controller.handleInput();

      expect(hooks.onTurnComplete).toHaveBeenCalledTimes(1);
      expect(hooks.onUpdateButtonStates).toHaveBeenCalledTimes(1);
    });

    it('works with no hooks provided', async () => {
      const minimalController = new ChatController(dom, state, {}, TEST_ICONS);
      dom.inputField.value = 'Hello';

      // Should not throw
      await minimalController.handleInput();
      expect(state.isProcessing).toBe(false);
    });
  });

  // ── Payload Construction ──────────────────────────────────────────

  describe('payload construction', () => {
    it('sends text-only payload when no images', async () => {
      let capturedPayload: any = null;
      mockIPC((cmd, args) => {
        if (cmd === 'chat') {
          capturedPayload = args;
          return null;
        }
      });
      controller = new ChatController(dom, state, hooks, TEST_ICONS);

      dom.inputField.value = 'Just text';
      await controller.handleInput();

      expect(capturedPayload.message).toBe('Just text');
      expect(capturedPayload.imagesBase64).toBeUndefined();
      expect(capturedPayload.imagesMimeTypes).toBeUndefined();
    });

    it('includes image data in payload when images attached', async () => {
      let capturedPayload: any = null;
      mockIPC((cmd, args) => {
        if (cmd === 'chat') {
          capturedPayload = args;
          return null;
        }
      });
      controller = new ChatController(dom, state, hooks, TEST_ICONS);

      state.attachedImages = [makeImage('a'), makeImage('b')];
      dom.inputField.value = 'With images';
      await controller.handleInput();

      expect(capturedPayload.imagesBase64).toEqual(['base64-a', 'base64-b']);
      expect(capturedPayload.imagesMimeTypes).toEqual(['image/png', 'image/png']);
    });

    it('uses lastUserMessage for skipUi payload', async () => {
      let capturedPayload: any = null;
      mockIPC((cmd, args) => {
        if (cmd === 'chat') {
          capturedPayload = args;
          return null;
        }
      });
      controller = new ChatController(dom, state, hooks, TEST_ICONS);

      state.lastUserMessage = 'original message';
      dom.inputField.value = 'original message';
      await controller.handleInput(true);

      expect(capturedPayload.message).toBe('original message');
    });
  });
});
