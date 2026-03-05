import { describe, it, expect } from 'vitest';
import type { SessionSummary } from '../ui/sessions';

/**
 * Tests verify that the DOM-based session rendering approach used in main.ts
 * (createElement + textContent) is inherently XSS-safe. textContent never
 * parses HTML, so malicious payloads are rendered as plain text.
 */
describe('Sessions UI - DOM-based rendering', () => {
  function renderSessionItem(s: SessionSummary): HTMLElement {
    const item = document.createElement("div");
    item.className = "session-item";
    item.dataset.id = s.session_id;

    const titleEl = document.createElement("div");
    titleEl.className = "session-item-title";
    titleEl.textContent = s.title;

    const metaEl = document.createElement("div");
    metaEl.className = "session-item-meta";

    const dateSpan = document.createElement("span");
    dateSpan.textContent = new Date(s.date).toLocaleDateString();

    const summarySpan = document.createElement("span");
    summarySpan.className = "session-item-summary";
    summarySpan.textContent = s.summary !== "No summary available"
      ? s.summary.substring(0, 120) + (s.summary.length > 120 ? "..." : "")
      : "";

    metaEl.appendChild(dateSpan);
    metaEl.appendChild(summarySpan);
    item.appendChild(titleEl);
    item.appendChild(metaEl);
    return item;
  }

  it('renders session items correctly', () => {
    const item = renderSessionItem({
      session_id: '123',
      title: 'Session 1',
      date: '2023-01-01T00:00:00.000Z',
      summary: 'Summary 1'
    });

    expect(item.dataset.id).toBe('123');
    expect(item.querySelector('.session-item-title')?.textContent).toBe('Session 1');
    expect(item.querySelector('.session-item-summary')?.textContent).toBe('Summary 1');
  });

  it('is safe against XSS in title (textContent never parses HTML)', () => {
    const item = renderSessionItem({
      session_id: 'xss',
      title: '<script>alert("xss")</script>',
      date: '2023-01-01T00:00:00.000Z',
      summary: 'Safe'
    });

    const titleEl = item.querySelector('.session-item-title')!;
    expect(titleEl.textContent).toBe('<script>alert("xss")</script>');
    expect(titleEl.innerHTML).not.toContain('<script>');
  });

  it('is safe against XSS in summary', () => {
    const item = renderSessionItem({
      session_id: 'xss',
      title: 'Safe',
      date: '2023-01-01T00:00:00.000Z',
      summary: '<img src=x onerror=alert(1)>'
    });

    const summaryEl = item.querySelector('.session-item-summary')!;
    expect(summaryEl.textContent).toBe('<img src=x onerror=alert(1)>');
    expect(summaryEl.innerHTML).not.toContain('<img');
  });

  it('is safe against XSS in session_id (dataset escapes automatically)', () => {
    const item = renderSessionItem({
      session_id: '"><script>alert(1)</script>',
      title: 'Safe',
      date: '2023-01-01T00:00:00.000Z',
      summary: 'Safe'
    });

    expect(item.dataset.id).toBe('"><script>alert(1)</script>');
    expect(item.outerHTML).not.toContain('data-id=""><script>');
  });

  it('truncates long summaries', () => {
    const longSummary = 'a'.repeat(200);
    const item = renderSessionItem({
      session_id: 'long',
      title: 'Long',
      date: '2023-01-01T00:00:00.000Z',
      summary: longSummary
    });

    const text = item.querySelector('.session-item-summary')?.textContent ?? '';
    expect(text.length).toBeLessThanOrEqual(123); // 120 + "..."
    expect(text).toContain('...');
  });

  it('shows empty summary for "No summary available"', () => {
    const item = renderSessionItem({
      session_id: 'no-summary',
      title: 'Test',
      date: '2023-01-01T00:00:00.000Z',
      summary: 'No summary available'
    });

    expect(item.querySelector('.session-item-summary')?.textContent).toBe('');
  });
});
