import { beforeEach, describe, expect, it, vi } from 'vitest';
import { mockIPC, clearMocks } from '@tauri-apps/api/mocks';
import { addProactiveMessage, draftStatusText, mountAttentionPanel } from '../ui/messages';
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

  it('shows the exact persona markdown and filename as safe plain text', () => {
    const container = document.createElement('div');
    const markdown = '# Persona\n\n<script>do not run</script>\n\n- technical detail';
    addProactiveMessage(container, { ...pending, draft_payload: JSON.stringify({ name: 'crystallize_sketch', arguments: { logical_path: 'personas/reviewer.md', markdown, source_sketch_id: 's1' } }) });
    expect(container.querySelector('.persona-draft-review strong')?.textContent).toContain('personas/reviewer.md');
    expect(container.querySelector('.persona-draft-review pre')?.textContent).toBe(markdown);
    expect(container.querySelector('.persona-draft-review script')).toBeNull();
    expect(container.textContent).toContain('saves this exact Markdown text');
  });

  it('shows a saved execution result immediately after approval', async () => {
    mockIPC((command) => {
      if (command === 'approve_draft') return 'Saved';
      if (command === 'get_draft_status') return { approved: true, reviewed_at: 'now', execution_status: 'succeeded', execution_result: 'Saved the reviewed text.' };
    });
    const container = document.createElement('div');
    addProactiveMessage(container, pending);
    (container.querySelector('.approve') as HTMLButtonElement).click();
    await vi.waitFor(() => expect(container.querySelector('.execution-result')?.textContent).toContain('Saved the reviewed text.'));
    expect(container.querySelector('.approve')).toBeNull();
  });

  it('does not invent approval for an old unknown action or reopen a collapsed panel', async () => {
    mockIPC(() => ({ actions: [{ ...pending, reviewed_at: 'legacy', execution_status: 'unknown' }], plans: [] }));
    const host = document.createElement('div');
    const chat = document.createElement('div');
    host.appendChild(chat);
    const panel = mountAttentionPanel(host, chat);
    await panel.refresh();
    expect(host.textContent).toContain('Approval unconfirmed');
    expect(host.querySelector('button')).toBeNull();
    const details = host.querySelector('.attention-panel') as HTMLDetailsElement;
    details.open = false;
    await panel.refresh();
    expect(details.open).toBe(false);
  });

  it('retains attention items when a later refresh fails', async () => {
    let calls = 0;
    mockIPC((command) => {
      if (command === 'get_attention_items' && calls++ === 0) return {
        actions: [{ ...pending, approved: true, reviewed_at: 'now', execution_status: 'failed' }],
        plans: [{ root_id: 'p1', title: 'Ship migration', completed: 1, total: 3, next_action_id: 'a2', next_action_title: 'Verify data' }],
      };
      throw new Error('offline');
    });
    const host = document.createElement('div');
    const chat = document.createElement('div');
    host.appendChild(chat);
    const panel = mountAttentionPanel(host, chat);
    await panel.refresh();
    expect(host.textContent).toContain('Not completed · source session test');
    expect(host.textContent).toContain('1 of 3 completed');
    await panel.refresh();
    expect(host.textContent).toContain('Ship migration');
    expect(host.textContent).toContain('Previously loaded items are retained');
    expect(host.querySelector('.approve')).toBeNull();
  });
});
