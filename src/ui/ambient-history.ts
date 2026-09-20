import { invoke } from "@tauri-apps/api/core";
import "./ambient-capabilities.css";

export interface AmbientSession {
  session_id: string;
  title: string;
  summary: string | null;
  date: string;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function renderHistoryView(
  host: HTMLElement,
  onLoadSession: (sessionId: string) => Promise<void>,
  onDeleteSession: (sessionId: string) => Promise<void>,
): () => void {
  const events = new AbortController();
  let sessions: AmbientSession[] = [];
  let requestSequence = 0;
  let searchTimer: ReturnType<typeof setTimeout> | undefined;
  host.classList.add("ambient-capability");
  host.replaceChildren();

  const search = document.createElement("input");
  search.type = "search";
  search.placeholder = "Search recent conversations";
  search.setAttribute("aria-label", "Search recent conversations");
  const status = document.createElement("p");
  status.className = "ambient-capability-status";
  status.setAttribute("role", "status");
  const list = document.createElement("div");
  list.className = "ambient-capability-list";
  list.setAttribute("aria-busy", "true");
  list.textContent = "Loading conversations…";
  host.append(search, status, list);

  const render = () => {
    const query = search.value.trim().toLocaleLowerCase();
    const visible = sessions.filter((session) =>
      `${session.title} ${session.summary ?? ""}`.toLocaleLowerCase().includes(query),
    );
    list.replaceChildren();
    list.setAttribute("aria-busy", "false");
    status.textContent = query ? `${visible.length} matching conversation${visible.length === 1 ? "" : "s"}` : "";
    if (visible.length === 0) {
      const empty = document.createElement("p");
      empty.className = "ambient-capability-empty";
      empty.textContent = query ? "No matching conversations." : "No recent conversations yet.";
      list.append(empty);
      return;
    }
    for (const session of visible) {
      const row = document.createElement("div");
      row.className = "ambient-capability-session-row";
      const button = document.createElement("button");
      button.type = "button";
      button.className = "ambient-capability-session";
      const title = document.createElement("strong");
      title.textContent = session.title || "Untitled conversation";
      const summary = document.createElement("span");
      summary.className = "ambient-capability-copy";
      summary.textContent = session.summary ?? "";
      button.append(title, summary);
      const remove = document.createElement("button");
      remove.type = "button";
      remove.dataset.danger = "true";
      remove.setAttribute("aria-label", `Delete ${session.title || "untitled conversation"}`);
      remove.textContent = "Delete";
      button.addEventListener(
        "click",
        async () => {
          button.disabled = true;
          status.textContent = "Opening conversation…";
          try {
            await onLoadSession(session.session_id);
          } catch (error) {
            status.dataset.tone = "error";
            status.textContent = `Conversation was not opened: ${errorMessage(error)}`;
            button.disabled = false;
          }
        },
        { signal: events.signal },
      );
      remove.addEventListener(
        "click",
        async () => {
          if (remove.dataset.confirm !== "true") {
            remove.dataset.confirm = "true";
            remove.textContent = "Confirm";
            return;
          }
          remove.disabled = true;
          status.textContent = "Deleting conversation…";
          try {
            await onDeleteSession(session.session_id);
            sessions = sessions.filter((item) => item.session_id !== session.session_id);
            render();
            status.dataset.tone = "quiet";
            status.textContent = "Conversation deleted.";
          } catch (error) {
            status.dataset.tone = "error";
            status.textContent = `Conversation was not deleted: ${errorMessage(error)}`;
            remove.disabled = false;
          }
        },
        { signal: events.signal },
      );
      row.append(button, remove);
      list.append(row);
    }
  };

  const load = async () => {
    const sequence = ++requestSequence;
    list.setAttribute("aria-busy", "true");
    status.dataset.tone = "quiet";
    status.textContent = search.value.trim() ? "Searching conversations…" : "";
    try {
      const result = await invoke<string>("get_recent_sessions", {
        limit: 50,
        query: search.value.trim() || null,
      });
      if (sequence !== requestSequence) return;
      sessions = result === "No matching sessions found." ? [] : JSON.parse(result);
      render();
    } catch (error) {
      if (sequence !== requestSequence) return;
      list.setAttribute("aria-busy", "false");
      list.textContent = "Conversations could not be loaded.";
      status.dataset.tone = "error";
      status.textContent = errorMessage(error);
    }
  };

  search.addEventListener(
    "input",
    () => {
      render();
      if (searchTimer) clearTimeout(searchTimer);
      searchTimer = setTimeout(() => void load(), 180);
    },
    { signal: events.signal },
  );
  void load();

  return () => {
    requestSequence += 1;
    if (searchTimer) clearTimeout(searchTimer);
    events.abort();
    host.classList.remove("ambient-capability");
  };
}
