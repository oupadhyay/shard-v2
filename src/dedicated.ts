/**
 * dedicated.ts — Entry point for the Dedicated Chat Window ("Breakout Mode").
 *
 * Reuses all backend Tauri commands and shared UI modules from src/ui/.
 * Renders a wider, sidebar-first layout with session management.
 *
 * Feature parity with main.ts (ambient):
 * - Streaming with copy button, <think> tag handling, whitespace guard
 * - Clipboard image paste + backspace-to-remove
 * - Cron job display, proactive messages, provider fallback notifications
 * - Focus tracking for blur-state CSS
 * - OCR shortcut listener, error retry with history rewind
 * - Full settings including heartbeat dashboard & cooldown
 */

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { openUrl } from "@tauri-apps/plugin-opener";
import DOMPurify from "dompurify";
import "katex/dist/katex.min.css";

import type { AttachedImage, ChatMessage, AppConfig, OcrResult, ModelsResponse, ProactiveMessage } from "./types";
import { ChatState } from "./state";
import { ChatController } from "./chat";
import { EVENTS } from "./events";
import {
  md,
  clearKatexErrors,
  createThinkingElement,
  updateThinkingElement,
  createToolCallElement,
  updateToolResult,
  addMessage,
  addProactiveMessage,
  getOrCreateWebSearchContainer,
  resetWebSearchContainer,
  isWebSearchTool,
  createWebSearchQueryElement,
  updateWebSearchCount,
  createStreamingAssistantMessage,
  renderStreamingContent,
  shouldSkipStreamingChunk,
  RESEND_ICON,
  STOP_ICON,
  RETRY_ICON,
  SETTINGS_MODAL_HTML,
  initSettingsTabs,
  populateHeartbeatsPanel,
  populateModelDropdown,
  resizeImage,
  formatSessionDate,
  logger,
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
    logger.error("Failed to return to ambient:", e);
    isClosing = false; // allow retry on error
  }
}

closeBtn?.addEventListener("click", returnToAmbient);
ambientBtn?.addEventListener("click", returnToAmbient);

// Intercept Cmd+W / OS titlebar close — same returnToAmbient path.
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
    openUrl(anchor.href).catch((e: any) => logger.error(e));
  }
});

// ── Focus Tracking ────────────────────────────────────────────────────────────

(function () {
  const root = document.documentElement;

  function setFocused(focused: boolean) {
    root.classList.toggle("app-focused", focused);
    root.classList.toggle("app-unfocused", !focused);
  }

  setFocused(document.hasFocus());

  window.addEventListener("focus", () => setFocused(true));
  window.addEventListener("blur", () => setFocused(false));

  let lastFocus = document.hasFocus();
  setInterval(() => {
    const now = document.hasFocus();
    if (now !== lastFocus) {
      lastFocus = now;
      setFocused(now);
    }
  }, 2000);
})();

// ── Input Handling ────────────────────────────────────────────────────────────

// Chat turn controller — shared logic between ambient and dedicated windows
const chatController = new ChatController(
  { chatArea, inputField, stopBtn, imagePreviewContainerId: "dedicated-image-preview-container" },
  state,
  {
    onUserMessageRendered: () => updateNewChatButtonState(),
    onTurnComplete: () => loadSessions(),
  },
  { stop: STOP_ICON, resend: RESEND_ICON },
);

// Convenience alias so existing call sites (keydown, stopBtn, retry) read naturally
const handleInput = (skipUi = false) => chatController.handleInput(skipUi);

inputField?.addEventListener("input", () => {
  inputField.style.height = "auto";
  inputField.style.height = inputField.scrollHeight + "px";
});

inputField?.addEventListener("keydown", (e) => {
  if (e.key === "Enter" && !e.shiftKey) {
    e.preventDefault();
    handleInput();
  } else if (e.key === "Backspace" && inputField.value === "" && state.attachedImages.length > 0) {
    e.preventDefault();
    state.attachedImages.pop();
    const container = document.getElementById("dedicated-image-preview-container");
    if (container) {
      const lastPreview = container.lastElementChild;
      if (lastPreview) lastPreview.remove();
    }
  }
});

// ── Clipboard Image Paste ─────────────────────────────────────────────────────

inputField?.addEventListener("paste", async (e) => {
  const clipboardData = e.clipboardData;
  if (!clipboardData) return;

  const items = Array.from(clipboardData.items);
  const imageItem = items.find((item) => item.type.startsWith("image/"));

  if (imageItem) {
    e.preventDefault();

    const file = imageItem.getAsFile();
    if (!file) return;

    setTimeout(() => {
      const objectUrl = URL.createObjectURL(file);
      const mimeType = file.type;

      const ocrTask = async () => {
        try {
          const base64 = await new Promise<string>((resolve, reject) => {
            const reader = new FileReader();
            reader.onload = () => resolve((reader.result as string).split(",")[1]);
            reader.onerror = reject;
            reader.readAsDataURL(file);
          });

          imageData.base64 = base64;

          const resizedBase64 = await resizeImage(base64, mimeType, 1024);
          const text = await invoke<string>("ocr_image", { imageBase64: resizedBase64 });
          return text;
        } catch (e) {
          logger.error("OCR Process failed:", e);
          return "[OCR failed]";
        }
      };

      const imageData: AttachedImage = {
        base64: "",
        mimeType,
        ocrText: "[Processing...]",
        previewUrl: objectUrl,
        ocrPromise: ocrTask()
      };

      showImagePreview(imageData);

      if (imageData.ocrPromise) {
        imageData.ocrPromise.then(text => {
          imageData.ocrText = text;
        });
      }

      inputField.focus();
    }, 0);
  }
});

stopBtn?.addEventListener("click", async () => {
  if (stopBtn.dataset.mode === "resend") {
    // Remove failed assistant output from UI
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

    state.attachedImages = [...state.lastAttachedImages];
    inputField.value = state.lastUserMessage;
    try { await invoke("rewind_history"); } catch (e) { logger.error(e); }
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
  } catch (e) { logger.error(e); }
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
  } catch (e) { logger.error("OCR error:", e); }
});

// Listen for OCR trigger from global shortcut
listen(EVENTS.TRIGGER_OCR, async () => {
  inputField.focus();
  try {
    const result = await invoke<OcrResult>("perform_ocr_capture");
    if (result) {
      const ocrPromise = invoke<string>("ocr_image", { imageBase64: result.image_base64 });
      const newImage: AttachedImage = {
        base64: result.image_base64,
        mimeType: result.mime_type,
        ocrText: "[Processing...]",
        ocrPromise,
      };
      showImagePreview(newImage);

      ocrPromise.then(text => {
        newImage.ocrText = text;
      }).catch(err => {
        logger.error("OCR failed:", err);
        newImage.ocrText = "[OCR failed]";
      });

      inputField.focus();
    }
  } catch (error) {
    logger.error("OCR error:", error);
    inputField.focus();
  }
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
const heartbeatCooldownInput = settingsModal.querySelector("#heartbeat-cooldown") as HTMLInputElement;
const geminiKeyInput = settingsModal.querySelector("#gemini-key") as HTMLInputElement;
const openRouterKeyInput = settingsModal.querySelector("#openrouter-key") as HTMLInputElement;
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
    populateModelDropdown(backgroundModelInput, modelsResponse.background_models, config.background_model || "gemma-4-26b-a4b-it");

    if (geminiKeyInput) geminiKeyInput.value = config.gemini_api_key || "";
    if (openRouterKeyInput) openRouterKeyInput.value = config.openrouter_api_key || "";
    if (groqKeyInput) groqKeyInput.value = config.groq_api_key || "";
    if (braveKeyInput) braveKeyInput.value = config.brave_api_key || "";

    if (enableToolsCheckbox) enableToolsCheckbox.checked = config.enable_tools || false;
    if (incognitoModeCheckbox) incognitoModeCheckbox.checked = config.incognito_mode || false;
    if (enableScreenContextCheckbox) {
      enableScreenContextCheckbox.checked = config.enable_screen_context || false;
      enableScreenContextCheckbox.disabled = incognitoModeCheckbox.checked;
    }
    if (heartbeatCooldownInput) {
      heartbeatCooldownInput.value = String(config.heartbeat_global_cooldown_secs ?? 60);
    }

    updateToolAvailability();
    checkProviderConflict();

    // Populate heartbeats dashboard (async, non-blocking)
    populateHeartbeatsPanel(settingsModal);
  } catch (e) {
    logger.error("Settings load error:", e);
  }
}

async function saveSettings() {
  const config = {
    gemini_api_key: geminiKeyInput.value || null,
    openrouter_api_key: openRouterKeyInput.value || null,
    groq_api_key: groqKeyInput.value || null,
    brave_api_key: braveKeyInput.value || null,
    selected_model: modelInput.value || null,
    background_model: backgroundModelInput.value || null,
    enable_web_search: true,
    enable_tools: enableToolsCheckbox.checked,
    incognito_mode: incognitoModeCheckbox.checked,
    enable_screen_context: enableScreenContextCheckbox.checked,
    heartbeat_global_cooldown_secs: parseInt(heartbeatCooldownInput?.value) || 60,
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

interface SessionEntry {
  session_id: string;
  title: string;
  summary: string;
  date: string;
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
      item.setAttribute("data-session-id", session.session_id);
      item.title = session.title;
      if (state.currentSessionId === session.session_id) {
        item.classList.add("active");
      }

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

          if (state.currentSessionId === session.session_id || item.classList.contains("active")) {
            state.currentSessionId = null;
            chatArea.innerHTML = "";
            state.currentThinkingBlock = null;

            const nextId = await invoke<string>("get_current_session_id");
            state.currentSessionId = nextId;
            updateNewChatButtonState();
          }

          await loadSessions();
        } catch (err) {
          logger.error("Failed to delete session:", err);
        }
      });

      item.appendChild(deleteBtn);
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
    if (typeof raw === "string" && raw === "No matching sessions found.") {
      allSessions = [];
    } else {
      try {
        allSessions = JSON.parse(raw) as SessionEntry[];
      } catch (err) {
        logger.error("Mismatched or invalid session JSON:", err);
        allSessions = [];
      }
    }
    renderSessions(allSessions);
  } catch (e) {
    logger.error("Failed to load sessions:", e);
  }
}

async function loadSession(sessionId: string) {
  try {
    await invoke("load_session", { sessionId });
    state.currentSessionId = sessionId;
    chatArea.innerHTML = "";
    state.currentThinkingBlock = null;
    await loadChatHistory();
    sessionsList.querySelectorAll(".sidebar-session-item").forEach((el) => {
      const elId = (el as HTMLElement).dataset.sessionId;
      el.classList.toggle("active", elId === sessionId);
    });
  } catch (e) {
    logger.error("Failed to load session:", e);
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
    updateNewChatButtonState();
    await loadSessions();
  } catch (e) { logger.error(e); }
});

// Listen for background session analysis completion
listen(EVENTS.SESSIONS_UPDATED, () => {
  loadSessions().catch(e => logger.error("Failed to reload sessions after update:", e));
});

// ── Load Chat History ─────────────────────────────────────────────────────────

async function loadChatHistory() {
  const fragment = document.createDocumentFragment();
  try {
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
        let matched = Array.from(fragment.querySelectorAll(".web-search-query"))
          .find((el) => el.getAttribute("data-tool-id") === msg.tool_call_id);
        if (!matched) {
          matched = Array.from(fragment.querySelectorAll(".tool-output"))
            .find((el) => el.getAttribute("data-tool-id") === msg.tool_call_id);
        }
        if (matched && msg.content) updateToolResult(matched, msg.content);
      }
    }
    chatArea.appendChild(fragment);
    chatArea.scrollTop = chatArea.scrollHeight;
    updateNewChatButtonState();

    // Load pending proactive messages at the end of chat history
    await loadProactiveMessages();
  } catch (e) {
    logger.error("Failed to load chat history:", e);
    if (fragment.hasChildNodes()) chatArea.appendChild(fragment);
  }
}

async function loadProactiveMessages() {
  try {
    const activeSessionId = await invoke<string>("get_current_session_id").catch(() => "");
    const messages = await invoke<ProactiveMessage[]>("get_proactive_messages");

    for (const msg of messages) {
      if (msg.heartbeat_session === activeSessionId) {
        addProactiveMessage(chatArea, msg);
      }
    }
  } catch (error) {
    logger.error("Failed to load proactive messages:", error);
  }
}

// ── Tauri Event Listeners ─────────────────────────────────────────────────────

listen<string>(EVENTS.AGENT_RESPONSE_CHUNK, (event) => {
  const chunk = event.payload;
  if (!chunk) return;

  let lastMsg = chatArea.lastElementChild;

  // Skip whitespace-only chunks that would create empty bubbles
  if (shouldSkipStreamingChunk(lastMsg, chunk)) return;

  // Remove loading indicator if present
  const loadingIndicator = chatArea.querySelector("#loading-indicator");
  if (loadingIndicator) loadingIndicator.remove();

  // Create new assistant message if needed
  if (
    !lastMsg ||
    !lastMsg.classList.contains("assistant") ||
    lastMsg.classList.contains("tool-output") ||
    lastMsg.classList.contains("thinking-output")
  ) {
    const msgDiv = createStreamingAssistantMessage();
    chatArea.appendChild(msgDiv);
    lastMsg = msgDiv;
  }

  const prev = lastMsg.getAttribute("data-raw") || "";
  const combined = prev + chunk;
  lastMsg.setAttribute("data-raw", combined);

  // Mark thinking complete if we have substantial content
  if (combined.length > 10) {
    const openThinking = chatArea.querySelector('.thinking-output:not([data-complete="true"])');
    if (openThinking) {
      updateThinkingElement(openThinking as HTMLElement, openThinking.getAttribute("data-thinking") || "", true);
    }
  }

  const html = renderStreamingContent(combined);

  const contentDiv = lastMsg.querySelector(".message-content");
  if (contentDiv) {
    contentDiv.innerHTML = html;
  } else {
    lastMsg.innerHTML = html;
  }
  chatArea.scrollTop = chatArea.scrollHeight;
});

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
    let matchingQuery = payload.id ? chatArea.querySelector(`[data-tool-id="${payload.id}"]`) as HTMLElement : null;

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
      const resultText = typeof result === "string" ? result : JSON.stringify(result, null, 2);
      updateToolResult(matchingQuery, resultText);
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

    chatArea.scrollTop = chatArea.scrollHeight;
  } catch (e) {
    logger.error("[Agent Retry] Failed to parse retry event:", e);
  }
});

// Listen for retry exhaustion (best-effort response stays, just clean up indicator)
listen<string>(EVENTS.AGENT_RETRY_EXHAUSTED, (event) => {
  try {
    const payload = JSON.parse(event.payload);
    logger.info("[Agent Retry] Retries exhausted:", payload);
    const loadingIndicator = chatArea.querySelector("#loading-indicator");
    if (loadingIndicator) loadingIndicator.remove();
  } catch (e) {
    logger.error("[Agent Retry] Failed to parse exhausted event:", e);
  }
});

// Listen for API errors and display with retry button
listen<string>(EVENTS.AGENT_ERROR, (event) => {
  const errorText = event.payload;
  logger.error("API Error:", errorText);

  // Remove loading indicator if present
  const loadingIndicator = chatArea.querySelector("#loading-indicator");
  if (loadingIndicator) loadingIndicator.remove();

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

  // Wire retry button — rewind history to avoid duplicating the failed user message
  const retryBtn = errorDiv.querySelector(".retry-btn");
  retryBtn?.addEventListener("click", async () => {
    errorDiv.remove();
    try { await invoke("rewind_history"); } catch (e) { logger.error("Failed to rewind history:", e); }
    state.attachedImages = [...state.lastAttachedImages];
    inputField.value = state.lastUserMessage;
    handleInput(true);
  });

  chatArea.appendChild(errorDiv);
  chatArea.scrollTop = chatArea.scrollHeight;

  // Reset processing state
  state.isProcessing = false;
  stopBtn.classList.remove("loading");
  stopBtn.style.display = "none";
});

// Listen for provider fallback notifications (rate limit → OpenRouter)
listen<string>(EVENTS.AGENT_FALLBACK, (event) => {
  if (state.fallbackShownThisTurn) return;
  state.fallbackShownThisTurn = true;

  try {
    const data = JSON.parse(event.payload);
    const title = data.title || "Provider Fallback";
    const details = data.details || "";

    logger.info("[Fallback]", title, details);

    const fallbackDiv = document.createElement("div");
    fallbackDiv.className = "message fallback-message";
    fallbackDiv.innerHTML = DOMPurify.sanitize(`
      <details class="fallback-accordion">
        <summary class="fallback-summary"></summary>
        <div class="fallback-details"></div>
      </details>
    `);
    const summaryEl = fallbackDiv.querySelector('.fallback-summary');
    if (summaryEl) summaryEl.textContent = title;
    const detailsEl = fallbackDiv.querySelector('.fallback-details');
    if (detailsEl) detailsEl.textContent = details;

    chatArea.appendChild(fallbackDiv);
    chatArea.scrollTop = chatArea.scrollHeight;
  } catch (e) {
    logger.error("Failed to parse fallback event:", e);
  }
});

// Listen for background cron jobs starting
listen<string>(EVENTS.AGENT_CRON_STARTED, (event) => {
  const prompt = DOMPurify.sanitize(event.payload);
  resetWebSearchContainer();
  addMessage(chatArea, "cron", prompt, undefined);
});

// Listen for incoming proactive messages/drafts
listen<ProactiveMessage>(EVENTS.PROACTIVE_MESSAGE, async (event) => {
  logger.info("[Proactive] Received new proactive action:", event.payload);

  const activeSessionId = await invoke<string>("get_current_session_id").catch(() => "");

  if (event.payload.heartbeat_session === activeSessionId) {
    addProactiveMessage(chatArea, event.payload);
  }
});

// ── Init ──────────────────────────────────────────────────────────────────────

async function init() {
  logger.info("[Init] Starting dedicated window initialization...");

  // 1. Sync current session ID from backend first
  try {
    state.currentSessionId = await invoke<string>("get_current_session_id");
    logger.info("[Init] Current session ID synced:", state.currentSessionId);
  } catch (e) {
    logger.error("[Init] Failed to sync session ID:", e);
  }

  // 2. Load chat history (uses the synced ID internally too, but safe)
  try {
    await loadChatHistory();
  } catch (e) {
    logger.error("[Init] Failed to load chat history:", e);
  }

  // 3. Load and render sessions (will highlight active one correctly)
  try {
    await loadSessions();
  } catch (e) {
    logger.error("[Init] Failed to load sessions:", e);
  }

  // 4. Populate settings (models, keys, etc.)
  try {
    await populateSettings();
  } catch (e) {
    logger.error("[Init] Failed to populate settings:", e);
  }

  updateNewChatButtonState();
  if (inputField) inputField.focus();
  logger.info("[Init] Dedicated window initialization complete.");
}

init();
