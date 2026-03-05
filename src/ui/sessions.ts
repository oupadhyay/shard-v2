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
