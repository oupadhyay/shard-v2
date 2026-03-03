/**
 * dedicated.ts — Entry point for the Dedicated Chat Window ("Breakout Mode").
 *
 * Reuses all backend Tauri commands and shared UI modules from src/ui/.
 * Renders a wider, sidebar-first layout with session management.
 */

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { openUrl } from "@tauri-apps/plugin-opener";
import DOMPurify from "dompurify";
import "katex/dist/katex.min.css";

import type { AttachedImage, ChatMessage, AppConfig, OcrResult } from "./types";
import { ChatState } from "./state";
import { EVENTS } from "./events";
import {
  md,
  clearKatexErrors,
  getKatexErrors,
  detectUnrenderedLatex,
  preprocessMarkdown,
  createThinkingElement,
  updateThinkingElement,
  createToolCallElement,
  updateToolResult,
  addMessage as addMessageToChat,
  getOrCreateWebSearchContainer,
  resetWebSearchContainer,
  isWebSearchTool,
  createWebSearchQueryElement,
  updateWebSearchCount,
  RESEND_ICON,
  STOP_ICON,
  SETTINGS_MODAL_HTML,
  initSettingsTabs,
} from "./ui";

// ── DOM References ────────────────────────────────────────────────────────────

const chatArea = document.getElementById("dedicated-chat-area") as HTMLDivElement;
const inputField = document.getElementById("dedicated-input-field") as HTMLTextAreaElement;
const stopBtn = document.getElementById("dedicated-stop-btn") as HTMLButtonElement;
const ocrBtn = document.getElementById("dedicated-ocr-btn") as HTMLButtonElement;
const settingsBtn = document.getElementById("dedicated-settings-btn") as HTMLButtonElement;
const ambientBtn = document.getElementById("dedicated-ambient-btn") as HTMLButtonElement;
const newChatBtn = document.getElementById("dedicated-new-chat-btn") as HTMLButtonElement;
const sessionsList = document.getElementById("dedicated-sessions-list") as HTMLDivElement;
const sessionSearch = document.getElementById("dedicated-session-search") as HTMLInputElement;
const minimizeBtn = document.getElementById("dedicated-minimize-btn") as HTMLButtonElement;
const closeBtn = document.getElementById("dedicated-close-btn") as HTMLButtonElement;
const settingsModal = document.getElementById("dedicated-settings-modal") as HTMLDivElement;

// ── State ─────────────────────────────────────────────────────────────────────

const state = new ChatState();

// ── Window Controls ───────────────────────────────────────────────────────────

const appWindow = getCurrentWindow();

minimizeBtn?.addEventListener("click", () => appWindow.minimize());


// isClosing prevents double-invoke: button → invoke → win.close() → onCloseRequested → invoke again
let isClosing = false;

async function returnToAmbient() {
  if (isClosing) return;
  isClosing = true;
  try {
    await invoke("close_dedicated_window");
  } catch (e) {
    console.error("Failed to return to ambient:", e);
    isClosing = false; // allow retry on error
  }
}

closeBtn?.addEventListener("click", returnToAmbient);
ambientBtn?.addEventListener("click", returnToAmbient);

// Intercept Cmd+W / OS titlebar close — same returnToAmbient path.
// When win.close() fires from Rust, onCloseRequested re-fires but isClosing=true,
// so we skip preventDefault and let the OS close proceed normally.
appWindow.onCloseRequested(async (event) => {
  if (isClosing) return; // second fire after Rust win.close() — let it through
  event.preventDefault();
  await returnToAmbient();
});
// ── Open external links in browser ───────────────────────────────────────────

document.addEventListener("click", (e) => {
  const target = e.target as HTMLElement;
  const anchor = target.closest("a");
  if (anchor?.href && (anchor.href.startsWith("http://") || anchor.href.startsWith("https://"))) {
    e.preventDefault();
    openUrl(anchor.href).catch(console.error);
  }
});

// ── Chat Helpers ──────────────────────────────────────────────────────────────

function addMessage(
  role: "user" | "assistant" | "cron",
  content: string,
  images?: { base64: string; mimeType: string }[],
  target: HTMLElement | DocumentFragment = chatArea,
) {
  addMessageToChat(target, role, content, images);
}

// ── Input Handling ────────────────────────────────────────────────────────────

async function handleInput(skipUi = false) {
  const text = inputField.value.trim();
  if ((!text && !skipUi) || state.isProcessing) return;

  state.resetForNewTurn();

  if (!skipUi) {
    state.lastUserMessage = text;
    state.isCancelled = false;
    const currentImages = [...state.attachedImages];
    state.lastAttachedImages = [...state.attachedImages];
    inputField.value = "";
    inputField.style.height = "auto";
    addMessage("user", text, currentImages);
    state.attachedImages = [];
    const container = document.getElementById("dedicated-image-preview-container");
    if (container) container.innerHTML = "";
  } else {
    state.isCancelled = false;
    inputField.value = "";
    inputField.style.height = "auto";
  }

  state.isProcessing = true;
  const imagesToSend = skipUi ? [...state.attachedImages] : [...state.lastAttachedImages];
  resetWebSearchContainer();
  clearKatexErrors();
  stopBtn.style.display = "inline-flex";
  stopBtn.classList.add("loading");
  stopBtn.innerHTML = STOP_ICON;
  stopBtn.dataset.mode = "stop";

  try {
    const payload: Record<string, unknown> = { message: skipUi ? state.lastUserMessage : text };

    if (imagesToSend.length > 0) {
      const config = await invoke<AppConfig>("get_config");
      const selectedModel = config?.selected_model || "";
      if (!selectedModel.toLowerCase().includes("gemini")) {
        await Promise.all(
          imagesToSend.map(async (img) => {
            if (img.ocrPromise) {
              try { img.ocrText = await img.ocrPromise; }
              catch { img.ocrText = "[OCR failed]"; }
            }
          })
        );
        const ocrTexts = imagesToSend.map((img) => img.ocrText).join("\n---\n");
        payload.message = `[Image OCR]:\n${ocrTexts}\n\n${payload.message}`;
      } else {
        payload.imagesBase64 = imagesToSend.map((img) => img.base64);
        payload.imagesMimeTypes = imagesToSend.map((img) => img.mimeType);
      }
    }

    await invoke("chat", payload);
  } catch (error) {
    console.error("Chat error:", error);
  } finally {
    state.isProcessing = false;
    stopBtn.classList.remove("loading");
    if (!state.isCancelled) stopBtn.style.display = "none";

    // Close any remaining open thinking block
    if (state.currentThinkingBlock && chatArea.contains(state.currentThinkingBlock)) {
      updateThinkingElement(state.currentThinkingBlock, state.currentThinkingBlock.getAttribute("data-thinking") || "", true);
      state.currentThinkingBlock = null;
    }

    if (!state.isCancelled) {
      const parseErrors = getKatexErrors();
      const allMessages = chatArea.querySelectorAll('.message.assistant:not(.tool-output):not(.thinking-output)');
      const lastAssistant = allMessages.length > 0 ? allMessages[allMessages.length - 1] : null;
      const responseText = lastAssistant?.getAttribute("data-raw") || "";
      const unrenderedErrors = detectUnrenderedLatex(responseText);
      const allErrors = [...parseErrors, ...unrenderedErrors];
      if (allErrors.length > 0) {
        try { await invoke("retry_with_katex_hint", { katexErrors: allErrors }); }
        catch (e) { console.error("[KaTeX] Retry failed:", e); }
      }
    }

    // Reset streaming state and refresh session sidebar
    currentAssistantEl = null;
    loadSessions();
  }
}

inputField?.addEventListener("input", () => {
  inputField.style.height = "auto";
  inputField.style.height = inputField.scrollHeight + "px";
});

inputField?.addEventListener("keydown", (e) => {
  if (e.key === "Enter" && !e.shiftKey) {
    e.preventDefault();
    handleInput();
  }
});

stopBtn?.addEventListener("click", async () => {
  if (stopBtn.dataset.mode === "resend") {
    state.attachedImages = [...state.lastAttachedImages];
    inputField.value = state.lastUserMessage;
    try { await invoke("rewind_history"); } catch (e) { console.error(e); }
    handleInput(true);
    return;
  }
  try {
    await invoke("cancel_current_stream");
    state.isCancelled = true;
    state.isProcessing = false;
    stopBtn.classList.remove("loading");
    stopBtn.innerHTML = RESEND_ICON;
    stopBtn.dataset.mode = "resend";
  } catch (e) { console.error(e); }
});

// ── OCR ───────────────────────────────────────────────────────────────────────

ocrBtn?.addEventListener("click", async () => {
  inputField.focus();
  try {
    const result = await invoke<OcrResult>("perform_ocr_capture");
    if (result) {
      const ocrPromise = invoke<string>("ocr_image", { imageBase64: result.image_base64 });
      showImagePreview({ base64: result.image_base64, mimeType: result.mime_type, ocrText: "[Processing...]", ocrPromise });
      const idx = state.attachedImages.length - 1;
      ocrPromise
        .then((text) => { if (state.attachedImages[idx]) state.attachedImages[idx].ocrText = text; })
        .catch(() => { if (state.attachedImages[idx]) state.attachedImages[idx].ocrText = "[OCR failed]"; });
      inputField.focus();
    }
  } catch (e) { console.error("OCR error:", e); }
});

function showImagePreview(imageData: AttachedImage) {
  state.attachedImages.push(imageData);
  const index = state.attachedImages.length - 1;
  let container = document.getElementById("dedicated-image-preview-container");
  if (!container) {
    container = document.createElement("div");
    container.id = "dedicated-image-preview-container";
    container.className = "image-preview-container";
    const inputWrap = inputField.parentElement;
    const bottomBar = inputWrap?.parentElement;
    if (bottomBar && inputWrap) bottomBar.insertBefore(container, inputWrap);
  }
  const preview = document.createElement("div");
  preview.className = imageData.ocrPromise ? "image-preview ocr-processing" : "image-preview";
  preview.dataset.index = index.toString();
  const imgSrc = imageData.previewUrl || `data:${imageData.mimeType};base64,${imageData.base64}`;
  preview.innerHTML = DOMPurify.sanitize(`<button class="image-close-btn" title="Remove image">×</button><img src="${imgSrc}" alt="Attached image ${index + 1}" />`);
  preview.querySelector(".image-close-btn")?.addEventListener("click", () => {
    const idx = parseInt(preview.dataset.index || "0");
    state.attachedImages.splice(idx, 1);
    preview.remove();
    container?.querySelectorAll(".image-preview").forEach((el, i) => {
      (el as HTMLElement).dataset.index = i.toString();
    });
  });
  container.appendChild(preview);
}

// ── Settings Modal ────────────────────────────────────────────────────────────

settingsModal.innerHTML = DOMPurify.sanitize(SETTINGS_MODAL_HTML);
initSettingsTabs(settingsModal);

async function populateSettings() {
  try {
    const config = await invoke<AppConfig>("get_config");
    const geminiInput = settingsModal.querySelector("#gemini-key") as HTMLInputElement | null;
    const orInput = settingsModal.querySelector("#openrouter-key") as HTMLInputElement | null;
    if (geminiInput && config.gemini_api_key) geminiInput.value = config.gemini_api_key;
    if (orInput && config.openrouter_api_key) orInput.value = config.openrouter_api_key;
  } catch (e) { console.error("Settings load error:", e); }
}

settingsBtn?.addEventListener("click", () => {
  settingsModal.classList.toggle("hidden");
  if (!settingsModal.classList.contains("hidden")) populateSettings();
});

settingsModal.querySelector("#close-settings")?.addEventListener("click", () => {
  settingsModal.classList.add("hidden");
});

// ── Session Sidebar ───────────────────────────────────────────────────────────

// Bug fix #2: Backend returns {session_id, title, summary, date} — not {id, created_at}
interface SessionEntry {
  session_id: string;
  title: string;
  summary: string;
  date: string; // e.g. "2026-02-28 07:42:10" — SQLite datetime string
}

let allSessions: SessionEntry[] = [];

// Bug fix #2: Parse SQLite datetime "YYYY-MM-DD HH:MM:SS" (UTC) without zone confusion
function formatSessionDate(raw: string): string {
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

function renderSessions(sessions: SessionEntry[]) {
  if (!sessionsList) return;
  sessionsList.innerHTML = "";

  if (sessions.length === 0) {
    const empty = document.createElement("div");
    empty.className = "sidebar-empty";
    empty.textContent = "No sessions yet";
    sessionsList.appendChild(empty);
    return;
  }

  const groups = new Map<string, SessionEntry[]>();
  for (const s of sessions) {
    const label = formatSessionDate(s.date);
    if (!groups.has(label)) groups.set(label, []);
    groups.get(label)!.push(s);
  }

  for (const [label, items] of groups) {
    const header = document.createElement("div");
    header.className = "sidebar-group-header";
    header.textContent = label;
    sessionsList.appendChild(header);

    for (const session of items) {
      const item = document.createElement("button");
      item.className = "sidebar-session-item";
      item.setAttribute("role", "listitem");
      // Bug fix #5: use session_id not id
      item.setAttribute("data-session-id", session.session_id);
      item.title = session.title;
      // Delete button (hover-revealed trash icon)
      const deleteBtn = document.createElement("button");
      deleteBtn.className = "sidebar-session-delete";
      deleteBtn.title = "Delete session";
      deleteBtn.setAttribute("aria-label", "Delete session");
      deleteBtn.innerHTML = `<svg xmlns="http://www.w3.org/2000/svg" width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="3 6 5 6 21 6"></polyline><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"></path></svg>`;
      deleteBtn.addEventListener("click", async (e) => {
        e.stopPropagation();
        try {
          await invoke("delete_session", { sessionId: session.session_id });
          await loadSessions();
          // If deleted session was active, clear chat
          if (item.classList.contains("active")) {
            chatArea.innerHTML = "";
            state.currentThinkingBlock = null;
            currentAssistantEl = null;
          }
        } catch (err) {
          console.error("Failed to delete session:", err);
        }
      });
      item.innerHTML = DOMPurify.sanitize(
        `<span class="session-title">${session.title || "Untitled"}</span>` +
        `<span class="session-preview">${session.summary || ""}</span>`
      );
      item.appendChild(deleteBtn);
      // Bug fix #5: pass session.session_id
      item.addEventListener("click", () => loadSession(session.session_id));
      sessionsList.appendChild(item);
    }
  }
}

async function loadSessions() {
  try {
    const raw = await invoke<string>("get_recent_sessions", { limit: 30 });
    // Backend returns "No matching sessions found." string on empty
    if (typeof raw === "string" && !raw.startsWith("[")) {
      allSessions = [];
    } else {
      allSessions = JSON.parse(raw) as SessionEntry[];
    }
    renderSessions(allSessions);
  } catch (e) {
    console.error("Failed to load sessions:", e);
  }
}

async function loadSession(sessionId: string) {
  try {
    await invoke("load_session", { sessionId });
    chatArea.innerHTML = "";
    state.currentThinkingBlock = null;
    currentAssistantEl = null;
    await loadChatHistory();
    sessionsList.querySelectorAll(".sidebar-session-item").forEach((el) =>
      el.classList.toggle("active", (el as HTMLElement).dataset.sessionId === sessionId)
    );
  } catch (e) {
    console.error("Failed to load session:", e);
  }
}

sessionSearch?.addEventListener("input", () => {
  const query = sessionSearch.value.toLowerCase().trim();
  if (!query) { renderSessions(allSessions); return; }
  renderSessions(allSessions.filter((s) =>
    (s.title || "").toLowerCase().includes(query) || (s.summary || "").toLowerCase().includes(query)
  ));
});

newChatBtn?.addEventListener("click", async () => {
  try {
    await invoke("save_and_clear_chat");
    chatArea.innerHTML = "";
    state.currentThinkingBlock = null;
    currentAssistantEl = null;
    await loadSessions();
  } catch (e) { console.error(e); }
});

// ── Load Chat History ─────────────────────────────────────────────────────────

async function loadChatHistory() {
  const fragment = document.createDocumentFragment();
  try {
    const history = await invoke<ChatMessage[]>("get_chat_history");
    chatArea.innerHTML = "";
    for (const msg of history) {
      const displayRole = msg.is_cron ? "cron" : msg.role;
      if (displayRole === "user" || displayRole === "cron") {
        resetWebSearchContainer();
        addMessage(displayRole as "user" | "assistant" | "cron", msg.content || "", msg.images, fragment);
      } else if (displayRole === "assistant") {
        if (msg.reasoning) fragment.appendChild(createThinkingElement(msg.reasoning, true));
        if (msg.tool_calls?.length) {
          msg.tool_calls.forEach((tc: any) => {
            const name = tc.function.name;
            if (isWebSearchTool(name)) {
              const c = getOrCreateWebSearchContainer(fragment);
              if (!fragment.contains(c)) fragment.appendChild(c);
              const qc = c.querySelector(".web-search-queries");
              if (qc) {
                let q = "";
                try { q = JSON.parse(tc.function.arguments).query || ""; } catch { q = tc.function.arguments; }
                qc.appendChild(createWebSearchQueryElement(q, tc.id));
                updateWebSearchCount(c);
              }
            } else {
              fragment.appendChild(createToolCallElement(name, tc.function.arguments, tc.id, false));
            }
          });
        }
        if (msg.content) addMessage("assistant", msg.content, msg.images, fragment);
      } else if (msg.role === "tool" && msg.tool_call_id) {
        const matched = Array.from(fragment.querySelectorAll(".tool-output"))
          .find((el) => el.getAttribute("data-tool-id") === msg.tool_call_id);
        if (matched && msg.content) updateToolResult(matched, msg.content);
      }
    }
    chatArea.appendChild(fragment);
    chatArea.scrollTop = chatArea.scrollHeight;
  } catch (e) {
    console.error("Failed to load chat history:", e);
    if (fragment.hasChildNodes()) chatArea.appendChild(fragment);
  }
}

// ── Tauri Event Listeners ─────────────────────────────────────────────────────

let currentAssistantEl: HTMLElement | null = null;

listen<string>(EVENTS.AGENT_RESPONSE_CHUNK, (event) => {
  const chunk = event.payload;
  if (!currentAssistantEl) {
    currentAssistantEl = document.createElement("div");
    currentAssistantEl.className = "message assistant";
    chatArea.appendChild(currentAssistantEl);
  }
  const prev = currentAssistantEl.getAttribute("data-raw") || "";
  const combined = prev + chunk;
  currentAssistantEl.setAttribute("data-raw", combined);
  currentAssistantEl.innerHTML = DOMPurify.sanitize(md.render(preprocessMarkdown(combined)));
  chatArea.scrollTop = chatArea.scrollHeight;
});

// Bug fix #3: Use state.currentThinkingBlock + updateThinkingElement (same pattern as main.ts)
listen<string>(EVENTS.AGENT_REASONING_CHUNK, (event) => {
  const content = event.payload;
  if (!state.currentThinkingBlock || !chatArea.contains(state.currentThinkingBlock)) {
    state.currentThinkingBlock = createThinkingElement(content, false);
    chatArea.appendChild(state.currentThinkingBlock);
  } else {
    const prev = state.currentThinkingBlock.getAttribute("data-thinking") || "";
    updateThinkingElement(state.currentThinkingBlock, prev + content, false);
  }
  chatArea.scrollTop = chatArea.scrollHeight;
});

// Bug fix #3: AGENT_TOOL_CALL payload is {name, args, id, rawArgs} — not {function:{name,arguments}}
listen<string>(EVENTS.AGENT_TOOL_CALL, (event) => {
  const payload = JSON.parse(event.payload);

  // Idempotent: update existing block if found by ID
  let toolDiv = payload.id ? chatArea.querySelector(`[data-tool-id="${payload.id}"]`) as HTMLElement | null : null;
  if (toolDiv) {
    const summaryArgs = toolDiv.querySelector(".tool-summary-args");
    if (summaryArgs) {
      const argsText = Object.entries(payload.args || {})
        .map(([k, v]) => `${md.utils.escapeHtml(k)}="${md.utils.escapeHtml(String(v))}"`)
        .join(" ");
      summaryArgs.textContent = argsText;
    }
    const toolArgs = toolDiv.querySelector(".tool-args");
    if (toolArgs) {
      toolArgs.textContent = payload.rawArgs || JSON.stringify(payload.args, null, 2);
    }
    return;
  }

  // Close any open thinking block before showing tool call
  if (state.currentThinkingBlock && chatArea.contains(state.currentThinkingBlock)) {
    updateThinkingElement(
      state.currentThinkingBlock,
      state.currentThinkingBlock.getAttribute("data-thinking") || "",
      true
    );
  }

  if (isWebSearchTool(payload.name)) {
    const container = getOrCreateWebSearchContainer(chatArea);
    if (!chatArea.contains(container)) chatArea.appendChild(container);
    const qc = container.querySelector(".web-search-queries");
    if (qc) {
      const query = payload.args?.query || "";
      qc.appendChild(createWebSearchQueryElement(query, payload.id));
      updateWebSearchCount(container);
    }
  } else {
    const newToolDiv = createToolCallElement(payload.name, JSON.stringify(payload.args), payload.id);
    chatArea.appendChild(newToolDiv);
  }
  chatArea.scrollTop = chatArea.scrollHeight;
});

listen<string>(EVENTS.AGENT_TOOL_RESULT, (event) => {
  const payload = JSON.parse(event.payload);
  const name = payload.name;
  const result = payload.result;

  if (isWebSearchTool(name)) {
    const webSearchQueries = Array.from(chatArea.querySelectorAll(".web-search-query"));
    const matchingQuery = webSearchQueries
      .reverse()
      .find((el) => {
        const resultSection = el.querySelector(".tool-result") as HTMLElement;
        return resultSection && resultSection.style.display === "none";
      });
    if (matchingQuery) {
      const resultSection = matchingQuery.querySelector(".tool-result") as HTMLElement;
      const resultContent = matchingQuery.querySelector(".tool-result-content");
      if (resultSection && resultContent) {
        const resultText = typeof result === "string" ? result : JSON.stringify(result, null, 2);
        const cleanResult = resultText
          .replace(/^Web Search Results:\n/, "")
          .split("\n")
          .filter((line: string) => line.trim().startsWith("-"))
          .map((line: string) => { const m = line.match(/^(- \[[^\]]+\]\([^)]+\))/); return m ? m[1] : line; })
          .join("\n");
        resultContent.innerHTML = DOMPurify.sanitize(md.render(preprocessMarkdown(cleanResult)));
        resultSection.style.display = "grid";
      }
    }
  } else {
    const toolDivs = Array.from(chatArea.querySelectorAll(".tool-output"));
    let matchingTool = payload.id
      ? toolDivs.find((el) => el.getAttribute("data-tool-id") === payload.id)
      : toolDivs[toolDivs.length - 1];
    if (!matchingTool) matchingTool = toolDivs[toolDivs.length - 1];
    if (matchingTool && result) updateToolResult(matchingTool, result);
  }
});

listen<string>(EVENTS.AGENT_RETRY, (event) => {
  try {
    JSON.parse(event.payload);
    while (chatArea.lastElementChild) {
      const el = chatArea.lastElementChild;
      if (el.classList.contains("user")) break;
      if (el.classList.contains("assistant") || el.classList.contains("tool-output") ||
        el.classList.contains("thinking-output") || el.classList.contains("web-search-container")) {
        el.remove();
      } else break;
    }
    resetWebSearchContainer();
    state.currentThinkingBlock = null;
    clearKatexErrors();
    currentAssistantEl = null;
  } catch (e) { console.error(e); }
});

// ── Init ──────────────────────────────────────────────────────────────────────

loadChatHistory();
loadSessions();
inputField?.focus();
