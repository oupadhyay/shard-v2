import { describe, it, expect, beforeEach, vi } from 'vitest';
import { mockIPC, clearMocks } from '@tauri-apps/api/mocks';
import { populateHeartbeatsPanel } from '../ui/settings';
import type { HeartbeatStatusInfo } from '../types';

describe('Settings UI Tests', () => {
  let container: HTMLElement;

  beforeEach(() => {
    clearMocks();
    vi.clearAllMocks();
    container = document.createElement('div');
    container.innerHTML = '<div id="heartbeat-list"></div>';
  });

  it('should display error message when get_heartbeat_status fails', async () => {
    mockIPC((cmd) => {
      if (cmd === 'get_heartbeat_status') {
        throw new Error('IPC Error');
      }
    });

    await populateHeartbeatsPanel(container);

    const listEl = container.querySelector('#heartbeat-list');
    expect(listEl?.innerHTML).toContain('Failed to load heartbeats.');
  });

  it('should display empty message when no heartbeats are configured', async () => {
    mockIPC((cmd) => {
      if (cmd === 'get_heartbeat_status') {
        return [];
      }
    });

    await populateHeartbeatsPanel(container);

    const listEl = container.querySelector('#heartbeat-list');
    expect(listEl?.innerHTML).toContain('No heartbeats configured.');
  });

  it('should render heartbeat cards when data is returned', async () => {
    const mockHeartbeats: HeartbeatStatusInfo[] = [
      {
        filename: 'test-heartbeat.toml',
        schedule: '0 0 * * *',
        session: 'test-session',
        persona: 'Test Persona',
        max_tool_calls: 5,
        max_runs_per_day: 1,
        prompt_preview: 'Test prompt preview',
      },
    ];

    mockIPC((cmd) => {
      if (cmd === 'get_heartbeat_status') {
        return mockHeartbeats;
      }
    });

    await populateHeartbeatsPanel(container);

    const listEl = container.querySelector('#heartbeat-list');
    expect(listEl?.innerHTML).toContain('test-heartbeat.toml');
    expect(listEl?.innerHTML).toContain('0 0 * * *');
    expect(listEl?.innerHTML).toContain('Test Persona');
    expect(listEl?.innerHTML).toContain('1/day');
  });
});
