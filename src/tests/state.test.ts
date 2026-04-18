import { describe, it, expect, beforeEach } from 'vitest';
import { ChatState } from '../state';
import type { AttachedImage } from '../types';

describe('ChatState', () => {
  let state: ChatState;

  beforeEach(() => {
    state = new ChatState();
  });

  it('should have correct initial values', () => {
    expect(state.isProcessing).toBe(false);
    expect(state.isCancelled).toBe(false);
    expect(state.currentSessionId).toBe(null);
    expect(state.attachedImages).toEqual([]);
    expect(state.lastAttachedImages).toEqual([]);
    expect(state.lastUserMessage).toBe('');
    expect(state.fallbackShownThisTurn).toBe(false);
    expect(state.currentThinkingBlock).toBe(null);
    expect(state.currentScreenContextImage).toBe(null);
    expect(state.currentSuggestions).toEqual([]);
    expect(state.suggestionTimeout).toBe(null);
  });

  it('should reflect state mutations', () => {
    state.isProcessing = true;
    state.isCancelled = true;
    state.currentSessionId = 'test-session-123';
    state.lastUserMessage = 'Hello, world!';

    const mockImage: AttachedImage = {
      base64: 'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8/5+hHgAHggJ/PchI7wAAAABJRU5ErkJggg==',
      mimeType: 'image/png',
      ocrText: 'mock ocr text'
    };
    state.attachedImages.push(mockImage);
    state.lastAttachedImages = [mockImage];

    expect(state.isProcessing).toBe(true);
    expect(state.isCancelled).toBe(true);
    expect(state.currentSessionId).toBe('test-session-123');
    expect(state.lastUserMessage).toBe('Hello, world!');
    expect(state.attachedImages).toHaveLength(1);
    expect(state.attachedImages[0]).toEqual(mockImage);
    expect(state.lastAttachedImages).toEqual([mockImage]);
  });

  it('should handle screen context image and suggestions', () => {
    const mockScreenImage = {
      base64: 'screen-base64',
      mimeType: 'image/png',
      ocrText: 'screen ocr'
    };
    state.currentScreenContextImage = mockScreenImage;
    state.currentSuggestions = ['Suggestion 1', 'Suggestion 2'];

    const timeout: ReturnType<typeof setTimeout> = setTimeout(() => {}, 1000);
    state.suggestionTimeout = timeout;

    try {
      expect(state.currentScreenContextImage).toEqual(mockScreenImage);
      expect(state.currentSuggestions).toEqual(['Suggestion 1', 'Suggestion 2']);
      expect(state.suggestionTimeout).toBe(timeout);
    } finally {
      clearTimeout(timeout);
    }
  });

  it('should reset turn-specific state in resetForNewTurn', () => {
    // Set properties that should be reset
    state.fallbackShownThisTurn = true;
    const mockDiv = document.createElement('div');
    state.currentThinkingBlock = mockDiv;

    // Set properties that should NOT be reset
    state.isProcessing = true;
    state.currentSessionId = 'active-session';
    state.attachedImages = [{ base64: '...', mimeType: '...', ocrText: '...' }];

    state.resetForNewTurn();

    // Assert reset properties
    expect(state.fallbackShownThisTurn).toBe(false);
    expect(state.currentThinkingBlock).toBe(null);

    // Assert non-reset properties
    expect(state.isProcessing).toBe(true);
    expect(state.currentSessionId).toBe('active-session');
    expect(state.attachedImages).toHaveLength(1);
  });
});
