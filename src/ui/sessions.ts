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

export function renderSessionItem(s: SessionSummary, formatDateFn: (d: string) => string): HTMLElement {
  const item = document.createElement("div");
  item.className = "session-item";
  item.dataset.id = s.session_id;

  const titleEl = document.createElement("div");
  titleEl.className = "session-item-title";
  // DOMPurify is not needed here: textContent does not parse HTML, so it is inherently XSS-safe.
  titleEl.textContent = s.title;

  const metaEl = document.createElement("div");
  metaEl.className = "session-item-meta";

  const dateSpan = document.createElement("span");
  dateSpan.textContent = formatDateFn(s.date);

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
