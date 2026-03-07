import { describe, it, expect } from 'vitest';
import { type SessionSummary, renderSessionItem } from '../ui/sessions';

/**
 * Tests verify that the DOM-based session rendering approach used in main.ts
 * (createElement + textContent) is inherently XSS-safe. textContent never
 * parses HTML, so malicious payloads are rendered as plain text.
 */
describe('Sessions UI - DOM-based rendering', () => {
  // Use a simple stub for date formatting in tests to avoid timezone issues
  // and focus on DOM sanitization/escaping logic.
  const formatDateStub = (d: string) => new Date(d).toLocaleDateString();

  function renderStubbedSessionItem(s: SessionSummary): HTMLElement {
    return renderSessionItem(s, formatDateStub);
  }

  it('renders session items correctly', () => {
    const item = renderStubbedSessionItem({
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
    const item = renderStubbedSessionItem({
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
    const item = renderStubbedSessionItem({
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
    const item = renderStubbedSessionItem({
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
    const item = renderStubbedSessionItem({
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
    const item = renderStubbedSessionItem({
      session_id: 'no-summary',
      title: 'Test',
      date: '2023-01-01T00:00:00.000Z',
      summary: 'No summary available'
    });

    expect(item.querySelector('.session-item-summary')?.textContent).toBe('');
  });
});
