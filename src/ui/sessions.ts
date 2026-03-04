import { md } from "./markdown";

export interface SessionSummary {
  session_id: string;
  title: string;
  date: string;
  summary: string;
}

export const SESSIONS_MODAL_HTML = `
  <div class="settings-content sessions-content-override">
    <div>
      <h3>Recent Sessions</h3>
    </div>
    <div class="sessions-list" id="sessions-list-container">
      <div class="loading-spinner">Loading...</div>
    </div>
    <div class="settings-actions">
      <button id="new-session-modal-btn">New Chat</button>
      <button id="close-sessions">Close</button>
    </div>
  </div>
`;

export function renderSessionList(sessions: SessionSummary[]): string {
  return sessions.map((s: SessionSummary) => {
    const escapedTitle = md.utils.escapeHtml(s.title || "");
    const date = new Date(s.date).toLocaleDateString();

    let summaryHtml = "";
    if (s.summary && s.summary !== "No summary available") {
      // Truncate first (on raw string) to avoid cutting entities
      const summary = s.summary.length > 120 ? s.summary.substring(0, 120) + "..." : s.summary;
      // Then escape
      summaryHtml = md.utils.escapeHtml(summary);
    }

    // Escape session ID just in case
    const escapedId = md.utils.escapeHtml(s.session_id || "");

    return `
      <div class="session-item" data-id="${escapedId}">
        <div class="session-item-title">${escapedTitle}</div>
        <div class="session-item-meta">
          <span>${date}</span>
          <span class="session-item-summary">${summaryHtml}</span>
        </div>
      </div>
    `;
  }).join("");
}
