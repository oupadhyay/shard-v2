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

import type { AttachedImage, ChatMessage, AppConfig, OcrResult, ModelsResponse } from "./types";
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
  addMessage,
  getOrCreateWebSearchContainer,
  resetWebSearchContainer,
  isWebSearchTool,
  createWebSearchQueryElement,
  updateWebSearchCount,
  RESEND_ICON,
  STOP_ICON,
  RETRY_ICON,
  SETTINGS_MODAL_HTML,
  initSettingsTabs,
  populateModelDropdown,
  formatSessionDate,
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

function updateNewChatButtonState() {
  if (newChatBtn) {
    newChatBtn.disabled = chatArea.children.length === 0;
  }
}

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
    addMessage(chatArea, "user", text, currentImages);
    updateNewChatButtonState();
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
    // Always send image binary data to backend — provider routing happens server-side
    const payload: Record<string, unknown> = { message: skipUi ? state.lastUserMessage : text };

    if (imagesToSend.length > 0) {
      payload.imagesBase64 = imagesToSend.map((img) => img.base64);
      payload.imagesMimeTypes = imagesToSend.map((img) => img.mimeType);
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
      const newImage: AttachedImage = {
        base64: result.image_base64,
        mimeType: result.mime_type,
        ocrText: "[Processing...]",
        ocrPromise
      };
      showImagePreview(newImage);

      ocrPromise
        .then((text) => { newImage.ocrText = text; })
        .catch(() => { newImage.ocrText = "[OCR failed]"; });
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
  if (imageData.ocrPromise) {
    imageData.ocrPromise.finally(() => {
      preview.classList.remove("ocr-processing");
    });
  }
  container.appendChild(preview);
}

// ── Settings Modal ────────────────────────────────────────────────────────────



settingsModal.innerHTML = DOMPurify.sanitize(SETTINGS_MODAL_HTML);
initSettingsTabs(settingsModal);

const modelInput = settingsModal.querySelector("#model-id") as HTMLSelectElement;
const backgroundModelInput = settingsModal.querySelector("#background-model-id") as HTMLSelectElement;
const providerConflictWarning = settingsModal.querySelector("#provider-conflict-warning") as HTMLDivElement;
const enableToolsCheckbox = settingsModal.querySelector("#enable-tools") as HTMLInputElement;
const incognitoModeCheckbox = settingsModal.querySelector("#incognito-mode") as HTMLInputElement;
const enableScreenContextCheckbox = settingsModal.querySelector("#enable-screen-context") as HTMLInputElement;
const geminiKeyInput = settingsModal.querySelector("#gemini-key") as HTMLInputElement;
const openRouterKeyInput = settingsModal.querySelector("#openrouter-key") as HTMLInputElement;
const cerebrasKeyInput = settingsModal.querySelector("#cerebras-key") as HTMLInputElement;
const groqKeyInput = settingsModal.querySelector("#groq-key") as HTMLInputElement;
const braveKeyInput = settingsModal.querySelector("#brave-key") as HTMLInputElement;

const UNSUPPORTED_TOOL_MODELS = [
  "allenai/olmo-3.1-32b-think:free"
];

const updateToolAvailability = () => {
  const selectedModel = modelInput.value;
  const isUnsupported = UNSUPPORTED_TOOL_MODELS.includes(selectedModel);

  if (isUnsupported) {
    enableToolsCheckbox.checked = false;
    enableToolsCheckbox.disabled = true;
  } else {
    enableToolsCheckbox.disabled = false;
  }
};

const getProvider = (selectEl: HTMLSelectElement): string | null => {
  const selectedOption = selectEl.options[selectEl.selectedIndex];
  return selectedOption?.dataset.provider || null;
};

const checkProviderConflict = () => {
  const chatProvider = getProvider(modelInput);
  const bgProvider = getProvider(backgroundModelInput);

  if (chatProvider && bgProvider && chatProvider === bgProvider) {
    providerConflictWarning.style.display = "block";
  } else {
    providerConflictWarning.style.display = "none";
  }
};



modelInput.addEventListener("change", () => {
  updateToolAvailability();
  checkProviderConflict();
});
backgroundModelInput.addEventListener("change", checkProviderConflict);

async function populateSettings() {
  try {
    const [modelsResponse, config] = await Promise.all([
      invoke<ModelsResponse>("get_available_models"),
      invoke<AppConfig>("get_config")
    ]);

    populateModelDropdown(modelInput, modelsResponse.chat_models, config.selected_model || "gemini-2.5-flash");
    populateModelDropdown(backgroundModelInput, modelsResponse.background_models, config.background_model || "gpt-oss-120b (Groq)");

    if (geminiKeyInput) geminiKeyInput.value = config.gemini_api_key || "";
    if (openRouterKeyInput) openRouterKeyInput.value = config.openrouter_api_key || "";
    if (cerebrasKeyInput) cerebrasKeyInput.value = config.cerebras_api_key || "";
    if (groqKeyInput) groqKeyInput.value = config.groq_api_key || "";
    if (braveKeyInput) braveKeyInput.value = config.brave_api_key || "";

    if (enableToolsCheckbox) enableToolsCheckbox.checked = config.enable_tools || false;
    if (incognitoModeCheckbox) incognitoModeCheckbox.checked = config.incognito_mode || false;
    if (enableScreenContextCheckbox) {
      enableScreenContextCheckbox.checked = config.enable_screen_context || false;
      enableScreenContextCheckbox.disabled = incognitoModeCheckbox.checked;
    }

    updateToolAvailability();
    checkProviderConflict();
  } catch (e) {
    console.error("Settings load error:", e);
  }
}

async function saveSettings() {
  const config = {
    gemini_api_key: geminiKeyInput.value || null,
    openrouter_api_key: openRouterKeyInput.value || null,
    cerebras_api_key: cerebrasKeyInput.value || null,
    groq_api_key: groqKeyInput.value || null,
    brave_api_key: braveKeyInput.value || null,
    selected_model: modelInput.value || null,
    background_model: backgroundModelInput.value || null,
    enable_web_search: true,
    enable_tools: enableToolsCheckbox.checked,
    incognito_mode: incognitoModeCheckbox.checked,
    enable_screen_context: enableScreenContextCheckbox.checked,
  };

  try {
    await invoke("save_config", { config });
    settingsModal.classList.add("hidden");
  } catch (e) {
    alert(`Failed to save settings: ${e}`);
  }
}

settingsModal.querySelector("#save-settings")?.addEventListener("click", saveSettings);

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
      const item = document.createElement("div");
      item.className = "sidebar-session-item";
      item.setAttribute("role", "button");
      item.setAttribute("tabindex", "0");
      // Bug fix #5: use session_id not id
      item.setAttribute("data-session-id", session.session_id);
      item.title = session.title;
      if (state.currentSessionId === session.session_id) {
        item.classList.add("active");
      }

      // Wrap title and preview in a container to avoid clobbering by sanitize
      const contentWrapper = document.createElement("div");
      contentWrapper.className = "sidebar-session-content";
      contentWrapper.innerHTML = DOMPurify.sanitize(
        `<span class="session-title">${session.title || "Untitled"}</span>` +
        `<span class="session-preview">${session.summary || ""}</span>`
      );
      item.appendChild(contentWrapper);

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
          console.log(`[Delete] Deleted session: ${session.session_id}`);

          // If deleted session was active, clear chat and fetch the new active session ID
          if (state.currentSessionId === session.session_id || item.classList.contains("active")) {
            console.log("[Delete] Deleted active session. Clearing chat area.");
            state.currentSessionId = null; // Clear immediately to avoid race
            chatArea.innerHTML = "";
            state.currentThinkingBlock = null;
            currentAssistantEl = null;

            // Fetch the new active session ID generated by the backend
            const nextId = await invoke<string>("get_current_session_id");
            state.currentSessionId = nextId;
            console.log(`[Delete] New active session ID: ${state.currentSessionId}`);
            updateNewChatButtonState();
          }

          // Reload sidebar list after state is updated
          await loadSessions();
        } catch (err) {
          console.error("Failed to delete session:", err);
        }
      });

      item.appendChild(deleteBtn);
      // Bug fix #5: pass session.session_id
      const handleSelect = () => loadSession(session.session_id);
      item.addEventListener("click", handleSelect);
      item.addEventListener("keydown", (e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          handleSelect();
        }
      });
      sessionsList.appendChild(item);
    }
  }
}

async function loadSessions() {
  try {
    const raw = await invoke<string>("get_recent_sessions", { limit: 30 });
    // Backend returns "No matching sessions found." string on empty
    // Backend returns "No matching sessions found." string on empty
    if (typeof raw === "string" && raw === "No matching sessions found.") {
      allSessions = [];
    } else {
      try {
        allSessions = JSON.parse(raw) as SessionEntry[];
      } catch (err) {
        console.error("Mismatched or invalid session JSON:", err);
        allSessions = [];
      }
    }
    renderSessions(allSessions);
  } catch (e) {
    console.error("Failed to load sessions:", e);
  }
}

async function loadSession(sessionId: string) {
  try {
    await invoke("load_session", { sessionId });
    state.currentSessionId = sessionId;
    chatArea.innerHTML = "";
    state.currentThinkingBlock = null;
    currentAssistantEl = null;
    await loadChatHistory();
    sessionsList.querySelectorAll(".sidebar-session-item").forEach((el) => {
      const elId = (el as HTMLElement).dataset.sessionId;
      el.classList.toggle("active", elId === sessionId);
    });
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
  if (newChatBtn.disabled) return;
  try {
    await invoke("save_and_clear_chat");
    state.currentSessionId = null;
    chatArea.innerHTML = "";
    state.currentThinkingBlock = null;
    currentAssistantEl = null;
    updateNewChatButtonState();
    // Pre-fetch immediately to show the "Active Session" stub, but real summary update
    // will arrive via `sessions-updated` event shortly after.
    await loadSessions();
  } catch (e) { console.error(e); }
});

// Listen for background session analysis completion
listen(EVENTS.SESSIONS_UPDATED, () => {
  loadSessions().catch(e => console.error("Failed to reload sessions after update:", e));
});

// ── Load Chat History ─────────────────────────────────────────────────────────

async function loadChatHistory() {
  const fragment = document.createDocumentFragment();
  try {
    // Sync current session ID from backend
    state.currentSessionId = await invoke<string>("get_current_session_id");

    const history = await invoke<ChatMessage[]>("get_chat_history");
    chatArea.innerHTML = "";
    for (const msg of history) {
      const displayRole = msg.is_cron ? "cron" : msg.role;
      if (displayRole === "user" || displayRole === "cron") {
        resetWebSearchContainer();
        addMessage(fragment, displayRole as "user" | "assistant" | "cron", msg.content || "", msg.images);
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
        if (msg.content) addMessage(fragment, "assistant", msg.content, msg.images);
      } else if (msg.role === "tool" && msg.tool_call_id) {
        const matched = Array.from(fragment.querySelectorAll(".tool-output"))
          .find((el) => el.getAttribute("data-tool-id") === msg.tool_call_id);
        if (matched && msg.content) updateToolResult(matched, msg.content);
      }
    }
    chatArea.appendChild(fragment);
    chatArea.scrollTop = chatArea.scrollHeight;
    updateNewChatButtonState();
  } catch (e) {
    console.error("Failed to load chat history:", e);
    if (fragment.hasChildNodes()) chatArea.appendChild(fragment);
  }
}

// ── Tauri Event Listeners ─────────────────────────────────────────────────────

let currentAssistantEl: HTMLElement | null = null;

listen<string>(EVENTS.AGENT_RESPONSE_CHUNK, (event) => {
  const chunk = event.payload;
  if (!chunk) return;

  const loadingIndicator = chatArea.querySelector("#loading-indicator");
  if (loadingIndicator && currentAssistantEl === loadingIndicator) {
    // If the current assistant element IS the loading indicator, clear it out
    // so we can start fresh.
    loadingIndicator.remove();
    currentAssistantEl = null;
  } else if (loadingIndicator) {
    loadingIndicator.remove();
  }

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
    if (isWebSearchTool(payload.name)) {
      const queryText = toolDiv.querySelector(".query-text");
      if (queryText) {
        const query = payload.args?.query || "";
        queryText.textContent = `"${query || 'Legacy Search'}"`;
      }
    } else {
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
    // 1. Try to find by ID if provided (preferred)
    let matchingQuery = payload.id ? chatArea.querySelector(`[data-tool-id="${payload.id}"]`) as HTMLElement : null;

    // 2. Fallback to finding the last search query without a result
    if (!matchingQuery) {
      const webSearchQueries = Array.from(chatArea.querySelectorAll(".web-search-query"));
      matchingQuery = webSearchQueries
        .reverse()
        .find((el) => {
          const resultSection = el.querySelector(".tool-result") as HTMLElement;
          return resultSection && resultSection.style.display === "none";
        }) as HTMLElement || null;
    }

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
    const payload = JSON.parse(event.payload);

    // Clear the failed response from UI before retry
    while (chatArea.lastElementChild) {
      const el = chatArea.lastElementChild;
      if (el.classList.contains("user")) break;
      if (
        el.classList.contains("assistant") ||
        el.classList.contains("tool-output") ||
        el.classList.contains("thinking-output") ||
        el.classList.contains("web-search-container")
      ) {
        el.remove();
      } else {
        break;
      }
    }

    resetWebSearchContainer();
    state.currentThinkingBlock = null;
    clearKatexErrors();

    // Show retrying indicator
    const retryingDiv = document.createElement("div");
    retryingDiv.id = "loading-indicator";
    retryingDiv.className = "message assistant";
    const escapedAttempt = md.utils.escapeHtml(String(payload.attempt));
    const escapedMax = md.utils.escapeHtml(String(payload.max));
    retryingDiv.innerHTML = `<span class="loading-dots">Retrying (${escapedAttempt}/${escapedMax})...</span>`;
    chatArea.appendChild(retryingDiv);

    // Ensure the stream knows the active element is the loading indicator so it clears it
    currentAssistantEl = retryingDiv;

    chatArea.scrollTop = chatArea.scrollHeight;
  } catch (e) {
    console.error("[Agent Retry] Failed to parse retry event:", e);
  }
});

// Listen for retry exhaustion (best-effort response stays, just clean up indicator)
listen<string>(EVENTS.AGENT_RETRY_EXHAUSTED, (event) => {
  try {
    const payload = JSON.parse(event.payload);
    console.log("[Agent Retry] Retries exhausted:", payload);
    const loadingIndicator = chatArea.querySelector("#loading-indicator");
    if (loadingIndicator) loadingIndicator.remove();
  } catch (e) {
    console.error("[Agent Retry] Failed to parse exhausted event:", e);
  }
});

// Listen for API errors and display with retry button
listen<string>(EVENTS.AGENT_ERROR, (event) => {
  const errorText = event.payload;
  console.error("API Error:", errorText);

  // Create error message with accordion and retry button below
  const errorDiv = document.createElement("div");
  errorDiv.className = "message error-message";
  errorDiv.innerHTML = DOMPurify.sanitize(`
    <details class="error-accordion">
      <summary class="error-summary">API Error</summary>
      <div class="error-details"></div>
    </details>
    <button class="retry-btn" title="Retry request">
      ${RETRY_ICON}
      <span>Retry</span>
    </button>
  `);
  const detailsEl = errorDiv.querySelector('.error-details');
  if (detailsEl) detailsEl.textContent = errorText;

  // Wire retry button
  const retryBtn = errorDiv.querySelector(".retry-btn");
  retryBtn?.addEventListener("click", async () => {
    errorDiv.remove();
    inputField.value = state.lastUserMessage;
    handleInput(false);
  });

  chatArea.appendChild(errorDiv);
  chatArea.scrollTop = chatArea.scrollHeight;
});

// ── Init ──────────────────────────────────────────────────────────────────────

async function init() {
  console.log("[Init] Starting dedicated window initialization...");

  // 1. Sync current session ID from backend first
  try {
    state.currentSessionId = await invoke<string>("get_current_session_id");
    console.log("[Init] Current session ID synced:", state.currentSessionId);
  } catch (e) {
    console.error("[Init] Failed to sync session ID:", e);
  }

  // 2. Load chat history (uses the synced ID internally too, but safe)
  try {
    await loadChatHistory();
  } catch (e) {
    console.error("[Init] Failed to load chat history:", e);
  }

  // 3. Load and render sessions (will highlight active one correctly)
  try {
    await loadSessions();
  } catch (e) {
    console.error("[Init] Failed to load sessions:", e);
  }

  // 4. Populate settings (models, keys, etc.)
  try {
    await populateSettings();
  } catch (e) {
    console.error("[Init] Failed to populate settings:", e);
  }

  updateNewChatButtonState();
  if (inputField) inputField.focus();
  console.log("[Init] Dedicated window initialization complete.");
}

init();
