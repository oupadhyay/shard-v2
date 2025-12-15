import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import DOMPurify from "dompurify";
import "katex/dist/katex.min.css";

// Internal modules
import type { AttachedImage, ChatMessage, OcrResult } from "./types";
import {
  md,
  createThinkingElement,
  createToolCallElement,
  updateToolResult,
  addMessage as addMessageToChat,
  RESEND_ICON,
  STOP_ICON,
  TRASH_ICON,
  UNDO_ICON,
  RETRY_ICON,
} from "./ui";

// DOM Elements
const chatArea = document.getElementById("chat-area") as HTMLDivElement;
const inputField = document.getElementById("input-field") as HTMLTextAreaElement;
const ocrBtn = document.getElementById("ocr-btn") as HTMLButtonElement;
const trashBtn = document.getElementById("trash-btn") as HTMLButtonElement;
const settingsBtn = document.getElementById("settings-btn") as HTMLButtonElement;
const stopBtn = document.getElementById("stop-btn") as HTMLButtonElement;

// State
let isProcessing = false;
let attachedImages: AttachedImage[] = [];
let lastUserMessage = "";
let lastAttachedImages: AttachedImage[] = [];
let isCancelled = false;

function addMessage(
  role: "user" | "assistant",
  content: string,
  images?: { base64: string; mimeType: string }[],
) {
  addMessageToChat(chatArea, role, content, images);
}

// Helper: Handle Input
async function handleInput(skipUi = false) {
  const text = inputField.value.trim();
  if ((!text && !skipUi) || isProcessing) return;

  // If skipping UI, we use the text passed in or the input value (which should be set by caller)
  // But actually, if skipUi is true, we expect the caller to have set inputField.value.
  // Let's stick to the plan: caller sets inputField.value.

  if (!skipUi) {
    lastUserMessage = text;
    isCancelled = false;

    // Capture current images state before clearing it
    const currentImages = [...attachedImages];
    lastAttachedImages = [...attachedImages]; // Save for resend

    inputField.value = "";
    inputField.style.height = "auto"; // Reset height
    // Pass all images for display
    addMessage("user", text, currentImages);
  } else {
    // Resending: reset cancelled state
    isCancelled = false;
    // We don't clear inputField here because we assume it was set for the logic but we don't want to clear it if it wasn't used?
    // Actually, handleInput clears it.
    inputField.value = "";
    inputField.style.height = "auto"; // Reset height
  }

  isProcessing = true;

  // Reset stop button to stop mode
  stopBtn.style.display = "inline-flex";
  stopBtn.classList.add("loading");
  stopBtn.innerHTML = STOP_ICON; // Ensure it shows stop icon
  stopBtn.dataset.mode = "stop";

  try {
    // Include image data or OCR text based on model
    // Use attachedImage if present, otherwise use lastAttachedImage if resending?
    // The plan said: Restore attachedImage = lastAttachedImage before calling handleInput(true)

    const messagePayload: any = { message: skipUi ? lastUserMessage : text };

    // Use the images that are currently set (restored by caller if resending)
    const imagesToSend = attachedImages;

    if (imagesToSend.length > 0) {
      // For OpenRouter models (don't support images), prepend OCR text
      // For Gemini models, send image data
      // Simple heuristic: if there's a slash in model name, it's OpenRouter
      const config = await invoke<any>("get_config");
      const selectedModel = config?.selected_model || "";

      if (selectedModel.includes("/")) {
        // OpenRouter - use OCR text for all images
        const ocrTexts = imagesToSend.map((img) => img.ocrText).join("\n---\n");
        messagePayload.message = `[Image OCR]:\n${ocrTexts}\n\n${messagePayload.message}`;
      } else {
        // Gemini - send image data as arrays
        messagePayload.imagesBase64 = imagesToSend.map((img) => img.base64);
        messagePayload.imagesMimeTypes = imagesToSend.map((img) => img.mimeType);
      }

      // Clear image preview after determining what to send
      attachedImages = [];
      const container = document.getElementById("image-preview-container");
      if (container) container.innerHTML = "";
    }

    console.log("Sending payload to backend:", {
      message: messagePayload.message,
      hasImage: !!messagePayload.imageBase64,
      imageLen: messagePayload.imageBase64?.length,
      mime: messagePayload.imageMimeType,
    });
    await invoke("chat", messagePayload);
    // Note: The actual response handling might need to be event-based if streaming
    // For now, assuming the command returns when done or we listen for events.
    // If `chat` returns void, we need to listen for "agent-response" events.
  } catch (error) {
    // API errors are handled by agent-error event listener
    // This catch handles network/Tauri invoke errors only
    console.error("Chat error:", error);
  } finally {
    isProcessing = false;
    stopBtn.classList.remove("loading"); // Remove loading state

    if (!isCancelled) {
      stopBtn.style.display = "none"; // Hide Stop button only if NOT cancelled
    }
  }

  // Update button states after message
  setTimeout(() => updateButtonStates(), 100);
}

stopBtn.addEventListener("click", async () => {
  if (stopBtn.dataset.mode === "resend") {
    // Remove the last assistant message (which was partial/cancelled)
    // AND any preceding tool/thinking outputs that belong to this generation
    while (chatArea.lastElementChild) {
      const el = chatArea.lastElementChild;
      // Stop if we hit a user message
      if (el.classList.contains("user")) break;

      if (
        el.classList.contains("assistant") ||
        el.classList.contains("tool-output") ||
        el.classList.contains("thinking-output")
      ) {
        el.remove();
      } else {
        // Stop if we encounter an unknown element type to be safe
        break;
      }
    }

    // Restore image state
    attachedImages = [...lastAttachedImages];

    // We don't need to set inputField.value if we use lastUserMessage inside handleInput
    // But handleInput reads inputField.value.
    // Let's set it so handleInput logic works, but pass skipUi=true
    inputField.value = lastUserMessage;

    // Sync backend history: remove the last turn so we don't duplicate
    try {
      await invoke("rewind_history");
      console.log("History rewound for resend");
    } catch (e) {
      console.error("Failed to rewind history:", e);
    }

    handleInput(true);
    return;
  }

  // Default "stop" behavior
  try {
    await invoke("cancel_current_stream");
    console.log("Cancellation requested");
    isCancelled = true;
    isProcessing = false;

    // Switch to Resend mode
    stopBtn.classList.remove("loading");
    stopBtn.innerHTML = RESEND_ICON;
    stopBtn.dataset.mode = "resend";
    // Do NOT hide the button
  } catch (e) {
    console.error("Failed to cancel stream:", e);
  }
});

// Auto-resize textarea
inputField.addEventListener("input", () => {
  inputField.style.height = "auto";
  inputField.style.height = inputField.scrollHeight + "px";
});

// Event Listeners: keydown for Enter to send and Backspace to remove image
inputField.addEventListener("keydown", (e) => {
  if (e.key === "Enter" && !e.shiftKey) {
    e.preventDefault();
    handleInput();
  } else if (e.key === "Backspace" && inputField.value === "" && attachedImages.length > 0) {
    e.preventDefault();
    // Remove last image
    attachedImages.pop();
    const container = document.getElementById("image-preview-container");
    if (container) {
      const lastPreview = container.lastElementChild;
      if (lastPreview) lastPreview.remove();
    }
  }
});

// Handle paste event for clipboard images
inputField.addEventListener("paste", async (e) => {
  const clipboardData = e.clipboardData;
  if (!clipboardData) return;

  // Check for image in clipboard
  const items = Array.from(clipboardData.items);
  const imageItem = items.find((item) => item.type.startsWith("image/"));

  if (imageItem) {
    e.preventDefault(); // Prevent default paste behavior for images

    const file = imageItem.getAsFile();
    if (!file) return;

    // Convert to base64
    const reader = new FileReader();
    reader.onload = () => {
      const base64 = (reader.result as string).split(",")[1]; // Remove data:image/...;base64, prefix
      const mimeType = file.type;

      // Show preview immediately with placeholder OCR (non-blocking)
      const imageData = {
        base64,
        mimeType,
        ocrText: "[Processing...]",
      };
      showImagePreview(imageData);
      const imageIndex = attachedImages.length - 1; // Index of just-added image

      // Run OCR in background (don't await)
      invoke<string>("ocr_image", { imageBase64: base64 })
        .then((ocrText) => {
          // Update the image's OCR text once complete
          if (attachedImages[imageIndex]) {
            attachedImages[imageIndex].ocrText = ocrText;
          }
        })
        .catch((err) => {
          console.warn("OCR failed for pasted image:", err);
          if (attachedImages[imageIndex]) {
            attachedImages[imageIndex].ocrText = "[OCR failed]";
          }
        });

      inputField.focus();
    };
    reader.readAsDataURL(file);
  }
});

function showImagePreview(imageData: { base64: string; mimeType: string; ocrText: string }) {
  // Add to images array
  attachedImages.push(imageData);
  const index = attachedImages.length - 1;

  let container = document.getElementById("image-preview-container");
  if (!container) {
    container = document.createElement("div");
    container.id = "image-preview-container";
    container.className = "image-preview-container";
    // Insert before the input-container (which contains the input field)
    const inputContainer = inputField.parentElement;
    const bottomBar = inputContainer?.parentElement;
    if (bottomBar && inputContainer) {
      bottomBar.insertBefore(container, inputContainer);
    }
  }

  // Create preview element for this image
  const preview = document.createElement("div");
  preview.className = "image-preview";
  preview.dataset.index = index.toString();
  preview.innerHTML = `
    <button class="image-close-btn" title="Remove image">×</button>
    <img src="data:${imageData.mimeType};base64,${imageData.base64}" alt="Attached image ${index + 1}" />
  `;

  // Add close handler
  preview.querySelector(".image-close-btn")?.addEventListener("click", () => {
    const idx = parseInt(preview.dataset.index || "0");
    attachedImages.splice(idx, 1);
    preview.remove();
    // Re-index remaining previews
    const remaining = container?.querySelectorAll(".image-preview") || [];
    remaining.forEach((el, i) => {
      (el as HTMLElement).dataset.index = i.toString();
    });
  });

  container.appendChild(preview);
}

ocrBtn.addEventListener("click", async () => {
  // Focus immediately so user can type while OCR processes
  inputField.focus();
  try {
    const result = await invoke<OcrResult>("perform_ocr_capture");
    if (result) {
      showImagePreview({
        base64: result.image_base64,
        mimeType: result.mime_type,
        ocrText: result.text,
      });
      // Do NOT paste text into input
      inputField.focus();
    }
  } catch (error) {
    console.error("OCR error:", error);
    alert(`OCR Failed: ${error}`);
    inputField.focus();
  }
});

// Listen for OCR trigger from global shortcut
listen("trigger-ocr", async () => {
  // Focus immediately so user can type while OCR processes
  inputField.focus();
  try {
    const result = await invoke<OcrResult>("perform_ocr_capture");
    if (result) {
      showImagePreview({
        base64: result.image_base64,
        mimeType: result.mime_type,
        ocrText: result.text,
      });
      inputField.focus();
    }
  } catch (error) {
    console.error("OCR error:", error);
    inputField.focus();
  }
});

// Delete/Undo State Management
async function updateButtonStates() {
  try {
    const messageCount = await invoke<number>("get_message_count");
    const hasBackup = await invoke<boolean>("has_backup");

    console.log("Button states:", { messageCount, hasBackup });

    // Disable button if no messages and no backup
    if (messageCount === 0 && !hasBackup) {
      trashBtn.disabled = true;
      trashBtn.dataset.mode = "delete";
      trashBtn.innerHTML = TRASH_ICON;
    } else if (hasBackup && messageCount === 0) {
      // Undo mode
      trashBtn.disabled = false;
      trashBtn.dataset.mode = "undo";
      trashBtn.title = "Undo Clear (Restore Chat)";
      trashBtn.innerHTML = UNDO_ICON;
    } else {
      // Delete mode
      trashBtn.disabled = false;
      trashBtn.dataset.mode = "delete";
      trashBtn.title = "Clear Chat";
      trashBtn.innerHTML = TRASH_ICON;
    }
  } catch (error) {
    console.error("Error updating button states:", error);
  }
}

trashBtn.addEventListener("click", async () => {
  const mode = trashBtn.dataset.mode;
  console.log("Trash clicked, mode:", mode);

  if (mode === "undo") {
    // Restore chat
    try {
      await invoke("restore_chat");
      location.reload();
    } catch (error) {
      console.error("Restore error:", error);
      alert(`Failed to restore: ${error}`);
    }
  } else {
    // Delete chat (no confirmation needed as we have undo)
    try {
      console.log("Calling save_and_clear_chat...");
      await invoke("save_and_clear_chat");
      console.log("Chat cleared, updating UI...");
      chatArea.innerHTML = "";
      await updateButtonStates();
      console.log("Button states updated");
    } catch (error) {
      console.error("Delete error:", error);
      alert(`Failed to delete: ${error}`);
    }
  }
});

// Update button states on page load
updateButtonStates();

async function loadChatHistory() {
  try {
    const history = await invoke<ChatMessage[]>("get_chat_history");
    chatArea.innerHTML = ""; // Clear existing

    // Process messages sequentially
    for (const msg of history) {
      if (msg.role === "user") {
        // Pass all images if present in history
        addMessage("user", msg.content || "", msg.images);
      } else if (msg.role === "assistant") {
        // 1. Render Reasoning (if present)
        if (msg.reasoning) {
          const thinkingMsg = createThinkingElement(msg.reasoning, true);
          chatArea.appendChild(thinkingMsg);
        }

        // 2. Render Tool Calls (if present)
        if (msg.tool_calls && msg.tool_calls.length > 0) {
          msg.tool_calls.forEach((toolCall: any) => {
            const toolDiv = createToolCallElement(
              toolCall.function.name,
              toolCall.function.arguments,
              toolCall.id,
              false, // Closed by default on restore
            );
            chatArea.appendChild(toolDiv);
          });
        }

        // 3. Render Assistant Content (if present)
        if (msg.content) {
          addMessage("assistant", msg.content, msg.images);
        }
      } else if (msg.role === "tool") {
        // Tool result message
        // Find the most recent tool-output that matches
        // We search backwards from the end of chatArea
        const toolMessages = Array.from(chatArea.querySelectorAll(".tool-output"));
        let matchingTool: Element | undefined;

        // Try to match by ID first if available
        if (msg.tool_call_id) {
          matchingTool = toolMessages
            .reverse()
            .find((el) => el.getAttribute("data-tool-id") === msg.tool_call_id);
        }

        // Fallback to name matching or just the last one if no ID
        if (!matchingTool) {
          matchingTool = toolMessages[toolMessages.length - 1];
        }

        if (matchingTool && msg.content) {
          updateToolResult(matchingTool, msg.content);
        }
      }
    }

    // Scroll to bottom
    chatArea.scrollTop = chatArea.scrollHeight;
  } catch (e) {
    console.error("Failed to load chat history:", e);
  }
}

loadChatHistory();

// Listen for agent streaming response chunks
listen<string>("agent-response-chunk", (event) => {
  const chunk = event.payload;

  // Ignore empty chunks
  if (!chunk) return;

  let lastMsg = chatArea.lastElementChild;

  // If we would create a new message, check if chunk is just whitespace
  // This prevents creating empty bubbles from leading newlines/spaces
  const isNewMessage =
    !lastMsg ||
    !lastMsg.classList.contains("assistant") ||
    lastMsg.classList.contains("tool-output") ||
    lastMsg.classList.contains("thinking-output");

  if (isNewMessage && chunk.trim().length === 0) {
    return;
  }

  // Remove loading indicator if present
  const loadingIndicator = chatArea.querySelector("#loading-indicator");
  if (loadingIndicator) {
    loadingIndicator.remove();
  }

  // Create or update assistant message (skip if last is thinking or tool)
  if (
    !lastMsg ||
    !lastMsg.classList.contains("assistant") ||
    lastMsg.classList.contains("tool-output") ||
    lastMsg.classList.contains("thinking-output")
  ) {
    const msgDiv = document.createElement("div");
    msgDiv.className = "message assistant markdown-body";
    chatArea.appendChild(msgDiv);
    lastMsg = msgDiv;
  }

  let rawText = lastMsg.getAttribute("data-raw") || "";
  rawText += chunk;
  lastMsg.setAttribute("data-raw", rawText);

  // Only mark thinking as complete if we have substantial content (> 10 chars)
  // This prevents marking it complete on the very first chunk (which might be interleaved with thinking)
  if (rawText.length > 10) {
    const openThinking = chatArea.querySelector('.thinking-output:not([data-complete="true"])');
    if (openThinking) {
      openThinking.setAttribute("data-complete", "true");
      // Change summary to "Thought" and close it
      const summary = openThinking.querySelector("summary");
      if (summary) summary.textContent = "Thought";
      const details = openThinking.querySelector("details");
      if (details) details.removeAttribute("open");
    }
  }

  let html = "";

  if (rawText.includes("<think>")) {
    // Handle <think> tags in content (for models that embed thinking in content)
    const openThink = rawText.indexOf("<think>");
    const closeThink = rawText.indexOf("</think>");

    if (closeThink !== -1) {
      // Closed thought
      const thought = rawText.substring(openThink + 7, closeThink);
      const rest = rawText.substring(closeThink + 8);
      html = `
            <details class="thought-block">
              <summary>Thought</summary>
              <div class="thought-content">${DOMPurify.sanitize(thought)}</div>
            </details>
            ${DOMPurify.sanitize(md.render(rest))}
          `;
    } else {
      // Open thought (still streaming)
      const thought = rawText.substring(openThink + 7);
      html = `
            <details class="thought-block" open>
              <summary>Thinking...</summary>
              <div class="thought-content">${DOMPurify.sanitize(thought)}</div>
            </details>
          `;
    }
  } else {
    // No thought tags, render normally
    html = DOMPurify.sanitize(md.render(rawText));
  }

  lastMsg.innerHTML = html;
  chatArea.scrollTop = chatArea.scrollHeight;
});

listen<string>("agent-reasoning-chunk", (event) => {
  // Handle explicit reasoning chunks - create separate message bubble like tool calls
  const content = event.payload;
  console.log("Received reasoning chunk:", content);

  // Check if we have an existing thinking message that's not complete
  let thinkingMsg: HTMLElement | null = null;
  const allMessages = chatArea.querySelectorAll(".message.thinking-output");

  // Find the last thinking message that's not marked as complete
  for (let i = allMessages.length - 1; i >= 0; i--) {
    const msg = allMessages[i] as HTMLElement;
    if (msg.getAttribute("data-complete") !== "true") {
      thinkingMsg = msg;
      break;
    }
  }

  if (!thinkingMsg) {
    // Create new thinking message bubble
    thinkingMsg = document.createElement("div");
    thinkingMsg.className = "message thinking-output";
    chatArea.appendChild(thinkingMsg);
  }

  let thinkingText = thinkingMsg.getAttribute("data-thinking") || "";
  thinkingText += content;
  thinkingMsg.setAttribute("data-thinking", thinkingText);

  // Render as standalone thinking block with Markdown
  // Trim trailing newlines for cleaner display
  const trimmedThinking = thinkingText.trimEnd();

  thinkingMsg.innerHTML = `
        <details class="thought-block" open>
          <summary>Thinking...</summary>
          <div class="thought-content markdown-body">${DOMPurify.sanitize(md.render(trimmedThinking))}</div>
        </details>
    `;

  chatArea.scrollTop = chatArea.scrollHeight;
});

listen<string>("agent-tool-call", (event) => {
  const payload = JSON.parse(event.payload);
  const toolDiv = createToolCallElement(payload.name, JSON.stringify(payload.args), payload.id);
  chatArea.appendChild(toolDiv);
  chatArea.scrollTop = chatArea.scrollHeight;
});

// Listen for tool results and add them to the matching tool call
listen<string>("agent-tool-result", (event) => {
  const payload = JSON.parse(event.payload);
  const name = payload.name;
  const result = payload.result;

  // Find the most recent tool-output with matching name
  const toolMessages = Array.from(chatArea.querySelectorAll(".tool-output"));
  const matchingTool = toolMessages
    .reverse()
    .find((el) => el.getAttribute("data-tool-name") === name);

  if (matchingTool) {
    updateToolResult(
      matchingTool,
      typeof result === "string" ? result : JSON.stringify(result, null, 2),
    );
  }
});

listen("agent-processing-start", () => {
  // Optional: Show a "thinking" indicator
});

// Listen for API errors and display with retry button
listen<string>("agent-error", (event) => {
  const errorText = event.payload;
  console.error("API Error:", errorText);

  // Remove loading indicator if present
  const loadingIndicator = chatArea.querySelector("#loading-indicator");
  if (loadingIndicator) {
    loadingIndicator.remove();
  }

  // Create error message with accordion and retry button below
  const errorDiv = document.createElement("div");
  errorDiv.className = "message error-message";
  errorDiv.innerHTML = `
    <details class="error-accordion">
      <summary class="error-summary">API Error</summary>
      <div class="error-details">${DOMPurify.sanitize(errorText)}</div>
    </details>
    <button class="retry-btn" title="Retry request">
      ${RETRY_ICON}
      <span>Retry</span>
    </button>
  `;

  // Wire retry button
  const retryBtn = errorDiv.querySelector(".retry-btn");
  retryBtn?.addEventListener("click", async () => {
    // Remove this error message
    errorDiv.remove();

    // Rewind backend history to remove the failed user message
    try {
      await invoke("rewind_history");
      console.log("History rewound for retry");
    } catch (e) {
      console.error("Failed to rewind history:", e);
    }

    // Restore last images and resend
    attachedImages = [...lastAttachedImages];
    inputField.value = lastUserMessage;
    handleInput(true);
  });

  chatArea.appendChild(errorDiv);
  chatArea.scrollTop = chatArea.scrollHeight;

  // Reset processing state
  isProcessing = false;
  stopBtn.classList.remove("loading");
  stopBtn.style.display = "none";
});
// Focus Tracking for Consistent Blur UI
(function () {
  const root = document.documentElement;

  function setFocused(focused: boolean) {
    root.classList.toggle("app-focused", focused);
    root.classList.toggle("app-unfocused", !focused);
  }

  // Initial state
  setFocused(document.hasFocus());

  window.addEventListener("focus", () => {
    setFocused(true);
  });
  window.addEventListener("blur", () => setFocused(false));

  // Edge case: some platforms can miss focus events after fast window switches
  // Polling fallback ensures correct state within a short interval.
  let lastFocus = document.hasFocus();
  setInterval(() => {
    const now = document.hasFocus();
    if (now !== lastFocus) {
      lastFocus = now;
      setFocused(now);
    }
  }, 500);
})();

// Window Visibility Logic
async function startHide() {
  const app = document.getElementById("app");
  if (app) {
    app.classList.add("hidden-app");
    // Wait for transition to finish (200ms)
    setTimeout(async () => {
      await invoke("hide_window");
    }, 200);
  }
}

listen("start-hide", () => {
  startHide();
});

listen("start-show", () => {
  const app = document.getElementById("app");
  if (app) {
    // Small delay to ensure window is rendered before fading in
    setTimeout(() => {
      app.classList.remove("hidden-app");
      // Focus input
      inputField.focus();
    }, 50);
  }
});

// Click-to-Hide Logic
document.addEventListener("click", (e) => {
  const target = e.target as HTMLElement;
  // Interactive elements: input container, messages, settings modal, bottom bar, buttons, image preview
  // We also check if the click was on a text selection? (Browser handles this, click fires after mouseup)
  // If user selects text, they might click? No, selection is drag. Click is click.

  const isInteractive = target.closest(
    ".input-container, .message, .settings-modal, .bottom-bar, .action-btn, .stop-btn, .image-preview",
  );

  if (!isInteractive) {
    startHide();
  }
});

// Settings Modal Logic
const settingsModal = document.createElement("div");
settingsModal.className = "settings-modal";
settingsModal.style.display = "none";
settingsModal.innerHTML = `
  <div class="settings-content">
    <h3>Settings</h3>
    <div class="setting-group">
      <label>Gemini API Key</label>
      <input type="password" id="gemini-key" placeholder="Enter Gemini API Key" />
    </div>
    <div class="setting-group">
      <label>OpenRouter API Key</label>
      <input type="password" id="openrouter-key" placeholder="Enter OpenRouter API Key" />
    </div>
    <div class="setting-group">
      <label>Brave Search API Key</label>
      <input type="password" id="brave-key" placeholder="Enter Brave API Key for web search" />
    </div>
    <div class="setting-group">
      <label>Model</label>
      <select id="model-id">
        <optgroup label="Gemini AI">
          <option value="gemini-2.5-flash-lite">gemini-2.5-flash-lite</option>
          <option value="gemini-2.5-flash">gemini-2.5-flash</option>
        </optgroup>
        <optgroup label="OpenRouter">
          <option value="google/gemma-3-27b-it:free">google/gemma-3-27b-it:free</option>
          <option value="openai/gpt-oss-20b:free">openai/gpt-oss-20b:free</option>
          <option value="mistralai/devstral-2512:free">mistralai/devstral-2512:free</option>
          <option value="allenai/olmo-3-32b-think:free">allenai/olmo-3-32b-think:free</option>
          <option value="meta-llama/llama-3.3-70b-instruct:free">meta-llama/llama-3.3-70b-instruct:free</option>
        </optgroup>
      </select>
    </div>
    <div class="setting-group">
      <label>
        <input type="checkbox" id="enable-tools" />
        Enable Tools (Weather, Search, etc.)
      </label>
    </div>
    <div class="setting-group">
      <label>
        <input type="checkbox" id="jailbreak-mode" />
        Jailbreak Mode (>500 tokens)
      </label>
    </div>
    <div class="settings-actions">
      <button id="save-settings">Save</button>
      <button id="close-settings">Close</button>
    </div>
  </div>
`;
document.body.appendChild(settingsModal);

const geminiKeyInput = document.getElementById("gemini-key") as HTMLInputElement;
const openRouterKeyInput = document.getElementById("openrouter-key") as HTMLInputElement;
const braveKeyInput = document.getElementById("brave-key") as HTMLInputElement;
const modelInput = document.getElementById("model-id") as HTMLSelectElement;
const enableToolsCheckbox = document.getElementById("enable-tools") as HTMLInputElement;
const jailbreakModeCheckbox = document.getElementById("jailbreak-mode") as HTMLInputElement;
const saveSettingsBtn = document.getElementById("save-settings") as HTMLButtonElement;
const closeSettingsBtn = document.getElementById("close-settings") as HTMLButtonElement;

// Define unsupported models (no tool calling support)
const UNSUPPORTED_TOOL_MODELS = [
  "allenai/olmo-3-32b-think:free",
  "meta-llama/llama-3.3-70b-instruct:free",
];

// Create warning element
const warningEl = document.createElement("div");
warningEl.style.color = "#ff4444";
warningEl.style.fontSize = "0.8em";
warningEl.style.marginTop = "5px";
warningEl.style.display = "none";
warningEl.textContent = "Tools are not supported for this model.";
enableToolsCheckbox.parentElement?.parentElement?.appendChild(warningEl);

const updateToolAvailability = () => {
  const selectedModel = modelInput.value;
  const isUnsupported = UNSUPPORTED_TOOL_MODELS.includes(selectedModel);

  if (isUnsupported) {
    enableToolsCheckbox.checked = false;
    enableToolsCheckbox.disabled = true;
    warningEl.style.display = "block";
  } else {
    enableToolsCheckbox.disabled = false;
    warningEl.style.display = "none";
  }
};

modelInput.addEventListener("change", updateToolAvailability);

settingsBtn.addEventListener("click", async () => {
  try {
    const config = await invoke<any>("get_config");
    geminiKeyInput.value = config.gemini_api_key || "";
    openRouterKeyInput.value = config.openrouter_api_key || "";
    braveKeyInput.value = config.brave_api_key || "";
    modelInput.value = config.selected_model || "gemini-2.5-flash";
    enableToolsCheckbox.checked = config.enable_tools || false;
    jailbreakModeCheckbox.checked = config.jailbreak_mode || false;

    updateToolAvailability(); // Run check on open

    settingsModal.style.display = "flex";
  } catch (e) {
    console.error("Failed to load config", e);
  }
});

closeSettingsBtn.addEventListener("click", () => {
  settingsModal.style.display = "none";
  inputField.focus();
});

saveSettingsBtn.addEventListener("click", async () => {
  const config = {
    gemini_api_key: geminiKeyInput.value || null,
    openrouter_api_key: openRouterKeyInput.value || null,
    brave_api_key: braveKeyInput.value || null,
    selected_model: modelInput.value || null,
    enable_web_search: true, // Default to true for now
    enable_tools: enableToolsCheckbox.checked,
    jailbreak_mode: jailbreakModeCheckbox.checked,
  };

  try {
    await invoke("save_config", { config });
    alert("Settings saved!");
    settingsModal.style.display = "none";
    inputField.focus();
  } catch (e) {
    alert(`Failed to save settings: ${e}`);
  }
});
