// Force UTC timezone for deterministic date comparisons
process.env.TZ = 'UTC';

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { formatSessionDate, logger } from '../ui/utils';

describe('logger', () => {
  let consoleSpy: any;
  let isDevSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    consoleSpy = {
      log: vi.spyOn(console, 'log').mockImplementation(() => {}),
      info: vi.spyOn(console, 'info').mockImplementation(() => {}),
      warn: vi.spyOn(console, 'warn').mockImplementation(() => {}),
      error: vi.spyOn(console, 'error').mockImplementation(() => {}),
    };
    isDevSpy = vi.spyOn(logger, 'isDev');
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('logs debug and info only in DEV mode', () => {
    isDevSpy.mockReturnValue(true);

    logger.debug('debug message');
    logger.info('info message');
    expect(consoleSpy.log).toHaveBeenCalledWith('debug message');
    expect(consoleSpy.info).toHaveBeenCalledWith('info message');

    consoleSpy.log.mockClear();
    consoleSpy.info.mockClear();

    // Switch to PROD mode
    isDevSpy.mockReturnValue(false);
    logger.debug('debug message');
    logger.info('info message');
    expect(consoleSpy.log).not.toHaveBeenCalled();
    expect(consoleSpy.info).not.toHaveBeenCalled();
  });

  it('always logs warn and error', () => {
    isDevSpy.mockReturnValue(false);

    logger.warn('warn message');
    logger.error('error message');
    expect(consoleSpy.warn).toHaveBeenCalledWith('warn message');
    expect(consoleSpy.error).toHaveBeenCalledWith('error message');
  });
});

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

  it('handles timestamp with T but no timezone/offset', () => {
    expect(formatSessionDate('2026-03-03T08:30:00', now)).toBe('Today');
  });

  it('handles RFC3339 with +00:00 offset', () => {
    expect(formatSessionDate('2026-03-03T08:30:00+00:00', now)).toBe('Today');
  });

  it('handles RFC3339 with non-UTC offset', () => {
    // -08:00 offset: 2026-03-02T20:00:00-08:00 = 2026-03-03T04:00:00Z → Today
    expect(formatSessionDate('2026-03-02T20:00:00-08:00', now)).toBe('Today');
  });

  it('returns "Unknown" for empty string', () => {
    expect(formatSessionDate('', now)).toBe('Unknown');
  });

  it('returns "Unknown" for invalid date', () => {
    expect(formatSessionDate('not-a-date', now)).toBe('Unknown');
  });
});
