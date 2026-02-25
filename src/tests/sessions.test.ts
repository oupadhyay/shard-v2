import { describe, it, expect } from 'vitest';
import { renderSessionList } from '../ui/sessions';

describe('Sessions UI', () => {
  describe('renderSessionList', () => {
    it('should render a list of sessions', () => {
      const sessions = [
        {
          session_id: '123',
          title: 'Session 1',
          date: '2023-01-01T00:00:00.000Z',
          summary: 'Summary 1'
        },
        {
          session_id: '456',
          title: 'Session 2',
          date: '2023-01-02T00:00:00.000Z',
          summary: 'Summary 2'
        }
      ];

      const html = renderSessionList(sessions);

      // Basic structure check
      expect(html).toContain('data-id="123"');
      expect(html).toContain('Session 1');
      expect(html).toContain('Summary 1');
      expect(html).toContain('data-id="456"');
      expect(html).toContain('Session 2');
      expect(html).toContain('Summary 2');
    });

    it('should escape HTML in title to prevent XSS', () => {
      const sessions = [
        {
          session_id: 'xss-title',
          title: '<script>alert("xss")</script>',
          date: '2023-01-01T00:00:00.000Z',
          summary: 'Safe summary'
        }
      ];

      const html = renderSessionList(sessions);

      expect(html).not.toContain('<script>');
      expect(html).toContain('&lt;script&gt;alert(&quot;xss&quot;)&lt;/script&gt;');
    });

    it('should escape HTML in summary to prevent XSS', () => {
      const sessions = [
        {
          session_id: 'xss-summary',
          title: 'Safe title',
          date: '2023-01-01T00:00:00.000Z',
          summary: '<img src=x onerror=alert(1)>'
        }
      ];

      const html = renderSessionList(sessions);

      expect(html).not.toContain('<img src=x');
      expect(html).toContain('&lt;img src=x onerror=alert(1)&gt;');
    });

    it('should escape HTML in session_id to prevent XSS', () => {
      const sessions = [
        {
          session_id: '"><script>alert(1)</script>',
          title: 'Safe title',
          date: '2023-01-01T00:00:00.000Z',
          summary: 'Safe summary'
        }
      ];

      const html = renderSessionList(sessions);

      // Should be escaped in data-id attribute
      expect(html).not.toContain('data-id=""><script>');
      expect(html).toContain('&quot;&gt;&lt;script&gt;');
    });

    it('should truncate long summaries', () => {
      const longSummary = 'a'.repeat(200);
      const sessions = [
        {
          session_id: 'long-summary',
          title: 'Long Summary',
          date: '2023-01-01T00:00:00.000Z',
          summary: longSummary
        }
      ];

      const html = renderSessionList(sessions);

      expect(html).toContain('...');
      // 120 chars + 3 dots = 123 chars max?
      // Implementation detail: substring(0, 120) + "..."
      // So checks it doesn't contain the full 200 chars
      expect(html).not.toContain(longSummary);
    });

    it('should handle "No summary available"', () => {
      const sessions = [
        {
          session_id: 'no-summary',
          title: 'No Summary',
          date: '2023-01-01T00:00:00.000Z',
          summary: 'No summary available'
        }
      ];

      const html = renderSessionList(sessions);

      // Should render empty string for summary
      expect(html).toContain('<span class="session-item-summary"></span>');
    });
  });
});
