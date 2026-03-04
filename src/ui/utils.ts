/**
 * Utility functions for UI formatting and shared logic
 */

/**
 * Formats a session date string into a human-readable relative label
 * @param raw SQLite datetime string (UTC)
 */
export function formatSessionDate(raw: string): string {
  if (!raw) return "Unknown";
  // Append "Z" to treat as UTC (SQLite stores UTC without timezone marker)
  const utcStr = raw.includes("T") ? raw : raw.replace(" ", "T") + "Z";
  const d = new Date(utcStr);
  if (isNaN(d.getTime())) return "Unknown";

  const now = new Date();
  // Compare calendar dates in local time
  const localDate = new Date(d.getFullYear(), d.getMonth(), d.getDate());
  const todayLocal = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const diffDays = Math.round((todayLocal.getTime() - localDate.getTime()) / (1000 * 60 * 60 * 24));

  if (diffDays === 0) return "Today";
  if (diffDays === 1) return "Yesterday";
  if (diffDays < 7) return "This Week";
  if (diffDays < 14) return "Last Week";
  return d.toLocaleDateString("en-US", { month: "long", year: "numeric" });
}
