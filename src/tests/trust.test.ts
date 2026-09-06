import { beforeEach, describe, expect, it, vi } from 'vitest';
import { mockIPC, clearMocks } from '@tauri-apps/api/mocks';
import { addProactiveMessage, draftStatusText } from '../ui/messages';
import type { ProactiveMessage } from '../types';

const pending: ProactiveMessage = {
  id: 'trust-test', heartbeat_session: 'test', content: 'Change settings',
  draft_payload: '{"name":"edit_file","arguments":{}}',
  needs_approval: true, approved: null, reviewed_at: null, created_at: '2026-09-06',
};

describe('durable approval UI', () => {
  beforeEach(() => clearMocks());

  it('does not call a legacy claimed draft dismissed or rejected', () => {
    expect(draftStatusText({ ...pending, reviewed_at: 'legacy' })).toContain('unconfirmed');
  });

  it.each(['unknown', 'failed', 'succeeded'] as const)('renders %s without replay buttons', (status) => {
    const container = document.createElement('div');
    addProactiveMessage(container, { ...pending, approved: true, reviewed_at: '2026-09-06', execution_status: status, execution_result: '<script>untrusted result</script>' });
    expect(container.querySelector('.approve')).toBeNull();
    expect(container.querySelector('.proactive-header')?.textContent).toContain('Reviewed Action');
    expect(container.querySelector('script')).toBeNull();
    expect(container.querySelector('.tool-args:last-child')).toBeTruthy();
    expect(container.textContent).toContain(status === 'unknown' ? 'unconfirmed' : status);
  });

  it('keeps controls disabled after uncertain IPC and reconciles saved failure', async () => {
    mockIPC((command) => {
      if (command === 'approve_draft') throw new Error('interrupted response');
      if (command === 'get_draft_status') return { approved: true, reviewed_at: '2026-09-06', execution_status: 'failed' };
    });
    const container = document.createElement('div');
    addProactiveMessage(container, pending);
    (container.querySelector('.approve') as HTMLButtonElement).click();
    await vi.waitFor(() => expect(container.textContent).toContain('execution failed'));
    expect(container.querySelector('.proactive-header')?.textContent).toContain('Reviewed Action');
    expect(container.querySelector('.approve')).toBeNull();
  });

  it('only enables retry when storage confirms the decision is still pending', async () => {
    mockIPC((command) => {
      if (command === 'approve_draft') throw new Error('claim failed');
      if (command === 'get_draft_status') return { approved: null, reviewed_at: null };
    });
    const container = document.createElement('div');
    addProactiveMessage(container, pending);
    const approve = container.querySelector('.approve') as HTMLButtonElement;
    approve.click();
    await vi.waitFor(() => expect(approve.disabled).toBe(false));
  });

  it('does not retry when both approval and status transport fail', async () => {
    mockIPC(() => { throw new Error('transport unavailable'); });
    const container = document.createElement('div');
    addProactiveMessage(container, pending);
    (container.querySelector('.approve') as HTMLButtonElement).click();
    await vi.waitFor(() => expect(container.textContent).toContain('Do not retry'));
    expect(container.querySelector('.approve')).toBeNull();
  });
});
