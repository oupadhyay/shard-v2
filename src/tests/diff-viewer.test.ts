import { describe, it, expect, beforeEach, vi } from 'vitest';
import { mountDiffViewer, type EditOutcome } from '../ui/diff-viewer';

function makeOutcome(overrides: Partial<EditOutcome> = {}): EditOutcome {
  return {
    path: 'config.toml',
    abs_path: '/tmp/config.toml',
    before: 'a\nb\nc\n',
    after: 'a\nB\nc\n',
    unified_diff: '--- a/config.toml\n+++ b/config.toml\n@@ -1,3 +1,3 @@\n a\n-b\n+B\n c\n',
    replacements: 1,
    ...overrides,
  };
}

describe('diff-viewer', () => {
  let host: HTMLElement;

  beforeEach(() => {
    document.body.innerHTML = '';
    host = document.createElement('div');
    document.body.appendChild(host);
  });

  it('mounts hidden until first edit arrives', () => {
    mountDiffViewer(host);
    const root = host.querySelector('.diff-viewer') as HTMLElement;
    expect(root).toBeTruthy();
    expect(root.hidden).toBe(true);
  });

  it('reveals + creates a tab for the first edit', () => {
    const ctrl = mountDiffViewer(host);
    ctrl.addOrUpdate(makeOutcome());
    const root = host.querySelector('.diff-viewer') as HTMLElement;
    expect(root.hidden).toBe(false);
    const tabs = host.querySelectorAll('.diff-viewer-tab');
    expect(tabs.length).toBe(1);
    expect(tabs[0].getAttribute('data-path')).toBe('config.toml');
    expect(tabs[0].classList.contains('active')).toBe(true);
  });

  it('reuses the tab when the same path is edited twice', () => {
    const ctrl = mountDiffViewer(host);
    ctrl.addOrUpdate(makeOutcome());
    ctrl.addOrUpdate(makeOutcome({ replacements: 2 }));
    expect(host.querySelectorAll('.diff-viewer-tab').length).toBe(1);
  });

  it('adds a new tab for a new path and shows correct add/del counts', () => {
    const ctrl = mountDiffViewer(host);
    ctrl.addOrUpdate(makeOutcome());
    ctrl.addOrUpdate(
      makeOutcome({
        path: 'heartbeats/news.toml',
        abs_path: '/tmp/news.toml',
        unified_diff:
          '--- a/news.toml\n+++ b/news.toml\n@@ -0,0 +1,2 @@\n+schedule = "0 * * * *"\n+prompt = "hi"\n',
      })
    );
    const tabs = host.querySelectorAll('.diff-viewer-tab');
    expect(tabs.length).toBe(2);
    // Latest tab is active.
    expect(tabs[1].classList.contains('active')).toBe(true);
    expect(tabs[0].classList.contains('active')).toBe(false);
    // Second tab's stats reflect 2 additions, 0 deletions.
    const newTabHtml = tabs[1].innerHTML;
    expect(newTabHtml).toContain('+2');
    expect(newTabHtml).toContain('-0');
  });

  it('streams diff lines into the active panel over time', async () => {
    vi.useFakeTimers();
    const ctrl = mountDiffViewer(host);
    ctrl.addOrUpdate(makeOutcome());
    const lines = host.querySelector('.diff-viewer-lines') as HTMLElement;
    // Before any timer has fired, the first batch may have been appended
    // synchronously inside the initial tick; advance time to drain remaining
    // batches.
    await vi.advanceTimersByTimeAsync(200);
    // All non-empty lines should be present; the unified diff has 7 lines.
    const rendered = lines.querySelectorAll('.diff-line');
    expect(rendered.length).toBeGreaterThanOrEqual(7);
    // Distinct classes — add, del, ctx, meta, hunk should all appear.
    const classes = new Set(
      Array.from(rendered).map((el) =>
        (el.className.match(/diff-(add|del|ctx|meta|hunk)/) || [])[0] || ''
      )
    );
    expect(classes.has('diff-add')).toBe(true);
    expect(classes.has('diff-del')).toBe(true);
    expect(classes.has('diff-ctx')).toBe(true);
    expect(classes.has('diff-meta')).toBe(true);
    expect(classes.has('diff-hunk')).toBe(true);
    vi.useRealTimers();
  });

  it('clicking a non-active tab switches the active panel', () => {
    const ctrl = mountDiffViewer(host);
    ctrl.addOrUpdate(makeOutcome());
    ctrl.addOrUpdate(makeOutcome({ path: 'other.toml' }));
    const firstTab = host.querySelector('.diff-viewer-tab[data-path="config.toml"]') as HTMLButtonElement;
    firstTab.click();
    expect(firstTab.classList.contains('active')).toBe(true);
    const panels = host.querySelectorAll('.diff-viewer-panel');
    const firstPanel = panels[0] as HTMLElement;
    const secondPanel = panels[1] as HTMLElement;
    expect(firstPanel.hidden).toBe(false);
    expect(secondPanel.hidden).toBe(true);
  });

  it('close button hides the viewer without clearing tabs', () => {
    const ctrl = mountDiffViewer(host);
    ctrl.addOrUpdate(makeOutcome());
    const root = host.querySelector('.diff-viewer') as HTMLElement;
    const closeBtn = host.querySelector('.diff-viewer-close') as HTMLButtonElement;
    closeBtn.click();
    expect(root.hidden).toBe(true);
    // Tabs persist across hide.
    expect(host.querySelectorAll('.diff-viewer-tab').length).toBe(1);
    // Re-show works.
    ctrl.setVisible(true);
    expect(root.hidden).toBe(false);
  });

  it('clear() removes all tabs and hides the panel', () => {
    const ctrl = mountDiffViewer(host);
    ctrl.addOrUpdate(makeOutcome());
    ctrl.addOrUpdate(makeOutcome({ path: 'second.toml' }));
    ctrl.clear();
    expect(host.querySelectorAll('.diff-viewer-tab').length).toBe(0);
    const root = host.querySelector('.diff-viewer') as HTMLElement;
    expect(root.hidden).toBe(true);
  });
});
