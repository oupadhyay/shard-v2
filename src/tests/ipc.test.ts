import { describe, it, expect, beforeEach, vi } from 'vitest';
import { mockIPC, clearMocks } from '@tauri-apps/api/mocks';
import { invoke } from '@tauri-apps/api/core';

// Mock window.crypto for UUID generation if needed
Object.defineProperty(window, 'crypto', {
  value: {
    getRandomValues: (buffer: any) => {
      return require('crypto').randomFillSync(buffer);
    },
  },
});

interface SaveConfigArgs {
  config: any;
}

interface ChatArgs {
  message: string;
  image_base64: string | null;
  image_mime_type: string | null;
}

describe('Shard IPC Tests', () => {
  beforeEach(() => {
    clearMocks();
    vi.clearAllMocks();
  });

  it('should load configuration correctly', async () => {
    const mockConfig = {
      theme: 'dark',
      selected_model: 'gemini-1.5-pro',
      gemini_api_key: 'test-key',
    };

    mockIPC((cmd) => {
      if (cmd === 'get_config') {
        return mockConfig;
      }
    });

    const config = await invoke('get_config');
    expect(config).toEqual(mockConfig);
  });

  it('should save configuration', async () => {
    let savedConfig: any = null;

    mockIPC((cmd, args) => {
      if (cmd === 'save_config') {
        savedConfig = (args as unknown as SaveConfigArgs).config;
        return null;
      }
    });

    const newConfig = { theme: 'light' };
    await invoke('save_config', { config: newConfig });
    expect(savedConfig).toEqual(newConfig);
  });

  it('should handle OCR capture', async () => {
    const mockOcrResult = {
      text: 'Detected Text',
      image_base64: 'base64data',
      mime_type: 'image/png',
    };

    mockIPC((cmd) => {
      if (cmd === 'perform_ocr_capture') {
        return mockOcrResult;
      }
    });

    const result = await invoke('perform_ocr_capture');
    expect(result).toEqual(mockOcrResult);
  });

  it('should handle chat messages', async () => {
    let sentMessage: any = null;

    mockIPC((cmd, args) => {
      if (cmd === 'chat') {
        sentMessage = (args as unknown as ChatArgs).message;
        return null; // Chat is streamed, but initial call returns null or similar
      }
    });

    await invoke('chat', { message: 'Hello AI', image_base64: null, image_mime_type: null });
    expect(sentMessage).toBe('Hello AI');
  });
});
