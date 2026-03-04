import { describe, it, expect } from 'vitest';
import { formatSessionDate } from '../ui/utils';

describe('formatSessionDate', () => {
  // Fixed reference: 2026-03-03 noon UTC
  const now = new Date('2026-03-03T12:00:00Z');

  it('returns "Today" for same-day dates', () => {
    expect(formatSessionDate('2026-03-03T08:30:00Z', now)).toBe('Today');
  });

  it('returns "Yesterday" for previous day', () => {
    expect(formatSessionDate('2026-03-02T23:59:00Z', now)).toBe('Yesterday');
  });

  it('returns "This Week" for 2-6 days ago', () => {
    expect(formatSessionDate('2026-02-28T10:00:00Z', now)).toBe('This Week');
    expect(formatSessionDate('2026-02-25T10:00:00Z', now)).toBe('This Week');
  });

  it('returns "Last Week" for 7-13 days ago', () => {
    expect(formatSessionDate('2026-02-24T10:00:00Z', now)).toBe('Last Week');
    expect(formatSessionDate('2026-02-19T10:00:00Z', now)).toBe('Last Week');
  });

  it('returns month + year for older dates', () => {
    expect(formatSessionDate('2026-01-15T10:00:00Z', now)).toBe('January 2026');
    expect(formatSessionDate('2025-12-01T10:00:00Z', now)).toBe('December 2025');
  });

  it('handles SQLite format without T or timezone', () => {
    expect(formatSessionDate('2026-03-03 08:30:00', now)).toBe('Today');
  });

  it('handles RFC3339 with +00:00 offset', () => {
    expect(formatSessionDate('2026-03-03T08:30:00+00:00', now)).toBe('Today');
  });

  it('handles RFC3339 with non-UTC offset', () => {
    // Verify it parses without error and returns a valid label
    const result = formatSessionDate('2026-03-02T20:00:00-08:00', now);
    expect(['Today', 'Yesterday']).toContain(result);
  });

  it('returns "Unknown" for empty string', () => {
    expect(formatSessionDate('', now)).toBe('Unknown');
  });

  it('returns "Unknown" for invalid date', () => {
    expect(formatSessionDate('not-a-date', now)).toBe('Unknown');
  });
});
