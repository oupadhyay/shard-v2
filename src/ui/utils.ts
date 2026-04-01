/**
 * Utility functions for UI formatting and shared logic
 */

/**
 * Formats a session date string into a human-readable relative label
 * @param raw SQLite/RFC3339 datetime string (typically UTC, may include offset)
 * @param now Optional reference date for testability (defaults to current time)
 */
export function formatSessionDate(raw: string, now?: Date): string {
  if (!raw) return "Unknown";
  // Normalize to RFC3339: support existing RFC3339-with-offset strings and
  // only append "Z" (UTC) when no timezone/offset is present.
  let normalized = raw.trim();
  if (!normalized.includes("T")) {
    normalized = normalized.replace(" ", "T");
  }
  const hasExplicitZone =
    normalized.endsWith("Z") || /[+-]\d{2}:?\d{2}$/.test(normalized);
  if (!hasExplicitZone) {
    normalized += "Z";
  }
  const d = new Date(normalized);
  if (isNaN(d.getTime())) return "Unknown";

  const ref = now ?? new Date();
  // Compare calendar dates in local time
  const localDate = new Date(d.getFullYear(), d.getMonth(), d.getDate());
  const todayLocal = new Date(ref.getFullYear(), ref.getMonth(), ref.getDate());
  const diffDays = Math.round((todayLocal.getTime() - localDate.getTime()) / (1000 * 60 * 60 * 24));

  if (diffDays === 0) return "Today";
  if (diffDays === 1) return "Yesterday";
  if (diffDays < 7) return "This Week";
  if (diffDays < 14) return "Last Week";
  return d.toLocaleDateString("en-US", { month: "long", year: "numeric" });
}

/**
 * Simple logger utility that only outputs to console in development mode
 * for debug and info levels. Warnings and errors are always shown.
 *
 * `isDev` is exposed as a method on `logger` for testability (can be spied on).
 */
export const logger = {
  isDev(): boolean {
    return (import.meta as any).env.DEV;
  },
  debug(...args: any[]) {
    if (this.isDev()) {
      console.log(...args);
    }
  },
  info(...args: any[]) {
    if (this.isDev()) {
      console.info(...args);
    }
  },
  warn(...args: any[]) {
    console.warn(...args);
  },
  error(...args: any[]) {
    console.error(...args);
  },
};
