import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import DOMPurify from "dompurify";
import "katex/dist/katex.min.css";

// Internal modules
import type { AttachedImage, ChatMessage, OcrResult, ChatMessagePayload, AppConfig } from "./types";
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
  TRASH_ICON,
  UNDO_ICON,
  RETRY_ICON,
  COPY_ICON,
  CHECK_ICON,
  SETTINGS_MODAL_HTML,
  SESSIONS_MODAL_HTML,
  initSettingsTabs,
  resizeImage,
} from "./ui";

// DOM Elements
const chatArea = document.getElementById("chat-area") as HTMLDivElement;
const inputField = document.getElementById("input-field") as HTMLTextAreaElement;
const ocrBtn = document.getElementById("ocr-btn") as HTMLButtonElement;
const trashBtn = document.getElementById("trash-btn") as HTMLButtonElement;
const settingsBtn = document.getElementById("settings-btn") as HTMLButtonElement;
const stopBtn = document.getElementById("stop-btn") as HTMLButtonElement;

// State (centralized in ChatState – see src/state.ts)
const state = new ChatState();

// Open external links in default browser
document.addEventListener("click", (e) => {
  const target = e.target as HTMLElement;
  const anchor = target.closest("a");
  if (anchor && anchor.href && (anchor.href.startsWith("http://") || anchor.href.startsWith("https://"))) {
    e.preventDefault();
    openUrl(anchor.href).catch(console.error);
  }
});

function addMessage(
  role: "user" | "assistant" | "cron",
  content: string,
  images?: { base64: string; mimeType: string }[],
  container: HTMLElement | DocumentFragment = chatArea,
) {
  addMessageToChat(container, role, content, images);
}

// Helper: Handle Input
async function handleInput(skipUi = false) {
  const text = inputField.value.trim();
  if ((!text && !skipUi) || state.isProcessing) return;

  // Reset per-turn state
  state.resetForNewTurn();

  // If skipping UI, we use the text passed in or the input value (which should be set by caller)
  // But actually, if skipUi is true, we expect the caller to have set inputField.value.
  // Let's stick to the plan: caller sets inputField.value.

  if (!skipUi) {
    state.lastUserMessage = text;
    state.isCancelled = false;

    // Capture current images state before clearing it
    const currentImages = [...state.attachedImages];
    state.lastAttachedImages = [...state.attachedImages]; // Save for resend

    inputField.value = "";
    inputField.style.height = "auto"; // Reset height
    // Pass all images for display
    addMessage("user", text, currentImages);

    // Clear image preview immediately to prevent duplication
    state.attachedImages = [];
    const container = document.getElementById("image-preview-container");
    if (container) container.innerHTML = "";
  } else {
    // Resending: reset cancelled state
    state.isCancelled = false;
    // We don't clear inputField here because we assume it was set for the logic but we don't want to clear it if it wasn't used?
    // Actually, handleInput clears it.
    inputField.value = "";
    inputField.style.height = "auto"; // Reset height
  }

  state.isProcessing = true;

  // Capture images for API call BEFORE any clearing.
  // Normal sends: use lastAttachedImages (snapshot taken above before clearing).
  // Resends (skipUi): attachedImages was restored by the caller.
  const imagesToSend = skipUi ? [...state.attachedImages] : [...state.lastAttachedImages];

  // Reset web search container for new response
  resetWebSearchContainer();


  // Clear KaTeX errors for new response (for auto-retry tracking)
  clearKatexErrors();

  // Reset stop button to stop mode
  stopBtn.style.display = "inline-flex";
  stopBtn.classList.add("loading");
  stopBtn.innerHTML = STOP_ICON; // Ensure it shows stop icon
  stopBtn.dataset.mode = "stop";

  try {
    // Include image data or OCR text based on model
    const messagePayload: ChatMessagePayload = { message: skipUi ? state.lastUserMessage : text };

    if (imagesToSend.length > 0) {
      // For OpenRouter models (don't support images), prepend OCR text
      // For Gemini models, send image data
      // Simple heuristic: if there's a slash in model name, it's OpenRouter
      const config = await invoke<AppConfig>("get_config");
      const selectedModel = config?.selected_model || "";

      // Heuristic: If it's not a Gemini model, we treat it as OpenRouter/Groq/etc and send OCR text
      if (!selectedModel.toLowerCase().includes("gemini")) {
        // Wait for any pending OCR
        const pendingImages = imagesToSend.filter(img => img.ocrPromise);
        if (pendingImages.length > 0) {
          await Promise.all(imagesToSend.map(async (img) => {
            if (img.ocrPromise) {
              try {
                const text = await img.ocrPromise;
                img.ocrText = text;
              } catch (e) {
                console.error("OCR failed during wait:", e);
                img.ocrText = "[OCR failed]";
              }
            }
          }));
        }

        // OpenRouter - use OCR text for all images
        const ocrTexts = imagesToSend.map((img) => img.ocrText).join("\n---\n");
        messagePayload.message = `[Image OCR]:\n${ocrTexts}\n\n${messagePayload.message}`;
      } else {
        // Gemini - send image data as arrays
        messagePayload.imagesBase64 = imagesToSend.map((img) => img.base64);
        messagePayload.imagesMimeTypes = imagesToSend.map((img) => img.mimeType);
      }
    }

    console.log("Sending payload to backend:", {
      message: messagePayload.message,
      hasImage: !!messagePayload.imagesBase64,
      imageLen: messagePayload.imagesBase64?.length,
      mime: messagePayload.imagesMimeTypes,
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
    state.isProcessing = false;
    stopBtn.classList.remove("loading"); // Remove loading state

    if (!state.isCancelled) {
      stopBtn.style.display = "none"; // Hide Stop button only if NOT cancelled
    }

    // Ensure any open thinking blocks are marked complete
    const openThinking = chatArea.querySelector('.thinking-output:not([data-complete="true"])');
    if (openThinking) {
      openThinking.setAttribute("data-complete", "true");
      const summary = openThinking.querySelector("summary");
      if (summary) summary.textContent = "Thought";
    }

    // Check for KaTeX errors (parse errors + unrendered LaTeX) and trigger retry if needed
    if (!state.isCancelled) {
      const parseErrors = getKatexErrors();

      // Find the last assistant message by iterating from the end
      const allMessages = chatArea.querySelectorAll('.message.assistant:not(.tool-output):not(.thinking-output)');
      const lastAssistant = allMessages.length > 0 ? allMessages[allMessages.length - 1] : null;
      const responseText = lastAssistant?.getAttribute('data-raw') || '';

      console.log("[KaTeX Check] Raw response text:", responseText.slice(0, 200));
      const unrenderedErrors = detectUnrenderedLatex(responseText);

      const allErrors = [...parseErrors, ...unrenderedErrors];

      if (allErrors.length > 0) {
        console.log("[KaTeX] Detected rendering issues, requesting retry:", allErrors);
        try {
          await invoke("retry_with_katex_hint", { katexErrors: allErrors });
        } catch (e) {
          console.error("[KaTeX] Retry request failed:", e);
        }
      }
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
    state.attachedImages = [...state.lastAttachedImages];

    // We don't need to set inputField.value if we use lastUserMessage inside handleInput
    // But handleInput reads inputField.value.
    // Let's set it so handleInput logic works, but pass skipUi=true
    inputField.value = state.lastUserMessage;

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
    state.isCancelled = true;
    state.isProcessing = false;

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
  } else if (e.key === "Backspace" && inputField.value === "" && state.attachedImages.length > 0) {
    e.preventDefault();
    // Remove last image
    state.attachedImages.pop();
    const container = document.getElementById("image-preview-container");
    if (container) {
      const lastPreview = container.lastElementChild;
      if (lastPreview) lastPreview.remove();
    }
  }
});

// Handle paste event for clipboard images
inputField.addEventListener("paste", async (e) => {
  console.log("[Paste] Event triggered");
  const clipboardData = e.clipboardData;
  if (!clipboardData) return;

  // Check for image in clipboard
  const items = Array.from(clipboardData.items);
  const imageItem = items.find((item) => item.type.startsWith("image/"));

  if (imageItem) {
    console.log("[Paste] Image item found in clipboard");
    e.preventDefault(); // Prevent default paste behavior for images

    const file = imageItem.getAsFile();
    if (!file) return;

    // Yield to main thread immediately to allow browser to handle event
    setTimeout(() => {
      // Create object URL for instant preview
      const objectUrl = URL.createObjectURL(file);
      const mimeType = file.type;

      // Predict index (since showImagePreview pushes)
      const imageIndex = state.attachedImages.length;

      // Define the async process immediately so the promise exists synchronously
      const ocrTask = async () => {
        try {
          // 1. Read file
          const base64 = await new Promise<string>((resolve, reject) => {
            const reader = new FileReader();
            reader.onload = () => resolve((reader.result as string).split(",")[1]);
            reader.onerror = reject;
            reader.readAsDataURL(file);
          });

          // Update base64 for Gemini (side effect)
          if (state.attachedImages[imageIndex]) {
            state.attachedImages[imageIndex].base64 = base64;
          }

          // 2. Resize
          console.log("[Paste] Resizing image for OCR...");
          const resizedBase64 = await resizeImage(base64, mimeType, 1024);

          // 3. Invoke OCR
          console.log("[Paste] Invoking ocr_image");
          const text = await invoke<string>("ocr_image", { imageBase64: resizedBase64 });

          console.log("[OCR] Success");
          return text;
        } catch (e) {
          console.error("OCR Process failed:", e);
          return "[OCR failed]";
        }
      };

      // Create image data with the promise attached immediately
      const imageData: AttachedImage = {
        base64: "", // Will be filled by side-effect
        mimeType,
        ocrText: "[Processing...]",
        previewUrl: objectUrl,
        ocrPromise: ocrTask() // Promise is created NOW
      };

      console.log("[Paste] Calling showImagePreview with ObjectURL");
      showImagePreview(imageData);

      // Add side-effect to update ocrText when done and remove loading state
      if (imageData.ocrPromise) {
        imageData.ocrPromise.then(text => {
          if (state.attachedImages[imageIndex]) {
            state.attachedImages[imageIndex].ocrText = text;
          }
          // Remove loading indicator from preview
          const container = document.getElementById("image-preview-container");
          const preview = container?.querySelector(`.image-preview[data-index="${imageIndex}"]`);
          if (preview) {
            preview.classList.remove("ocr-processing");
          }
        });
      }

      inputField.focus();
    }, 0);
  }
});

function showImagePreview(imageData: AttachedImage) {
  console.log("[showImagePreview] Called with mimeType:", imageData.mimeType);
  // Add to images array
  state.attachedImages.push(imageData);
  const index = state.attachedImages.length - 1;

  let container = document.getElementById("image-preview-container");
  if (!container) {
    console.log("[showImagePreview] Creating new container");
    container = document.createElement("div");
    container.id = "image-preview-container";
    container.className = "image-preview-container";
    // Insert before the input-container (which contains the input field)
    const inputContainer = inputField.parentElement;
    const bottomBar = inputContainer?.parentElement;
    if (bottomBar && inputContainer) {
      bottomBar.insertBefore(container, inputContainer);
    }
  } else {
    console.log("[showImagePreview] Container found");
  }

  // Create preview element for this image
  const preview = document.createElement("div");
  preview.className = imageData.ocrPromise ? "image-preview ocr-processing" : "image-preview";
  preview.dataset.index = index.toString();

  const imgSrc = imageData.previewUrl || `data:${imageData.mimeType};base64,${imageData.base64}`;

  preview.innerHTML = `
    <button class="image-close-btn" title="Remove image">×</button>
    <img src="${imgSrc}" alt="Attached image ${index + 1}" />
  `;

  // Add close handler
  preview.querySelector(".image-close-btn")?.addEventListener("click", () => {
    const idx = parseInt(preview.dataset.index || "0");
    state.attachedImages.splice(idx, 1);
    preview.remove();
    // Re-index remaining previews
    const remaining = container?.querySelectorAll(".image-preview") || [];
    remaining.forEach((el, i) => {
      (el as HTMLElement).dataset.index = i.toString();
    });
  });

  console.log("[showImagePreview] Appending preview to container");
  container.appendChild(preview);
  console.log("[showImagePreview] Append complete");
}

ocrBtn.addEventListener("click", async () => {
  // Focus immediately so user can type while OCR processes
  inputField.focus();
  try {
    const result = await invoke<OcrResult>("perform_ocr_capture");
    if (result) {
      // Create promise first so showImagePreview can detect it
      const ocrPromise = invoke<string>("ocr_image", { imageBase64: result.image_base64 });

      showImagePreview({
        base64: result.image_base64,
        mimeType: result.mime_type,
        ocrText: "[Processing...]",
        ocrPromise,
      });
      const index = state.attachedImages.length - 1;

      ocrPromise.then(text => {
        console.log("[OCR] Screenshot text:", text.substring(0, 50) + "...");
        if (state.attachedImages[index]) state.attachedImages[index].ocrText = text;
        // Remove loading indicator
        const container = document.getElementById("image-preview-container");
        const preview = container?.querySelector(`.image-preview[data-index="${index}"]`);
        if (preview) preview.classList.remove("ocr-processing");
      }).catch(err => {
        console.error("OCR failed:", err);
        if (state.attachedImages[index]) state.attachedImages[index].ocrText = "[OCR failed]";
        // Remove loading indicator
        const container = document.getElementById("image-preview-container");
        const preview = container?.querySelector(`.image-preview[data-index="${index}"]`);
        if (preview) preview.classList.remove("ocr-processing");
      });

      inputField.focus();
    }
  } catch (error) {
    console.error("OCR error:", error);
    const errorDiv = document.createElement("div");
    errorDiv.className = "message error-message";
    errorDiv.innerHTML = DOMPurify.sanitize(`
      <details class="error-accordion">
        <summary class="error-summary">OCR Error</summary>
        <div class="error-details"></div>
      </details>
    `);
    const detailsEl = errorDiv.querySelector('.error-details');
    if (detailsEl) detailsEl.textContent = String(error);
    chatArea.appendChild(errorDiv);
    chatArea.scrollTop = chatArea.scrollHeight;
    inputField.focus();
  }
});

// Listen for OCR trigger from global shortcut
listen(EVENTS.TRIGGER_OCR, async () => {
  // Focus immediately so user can type while OCR processes
  inputField.focus();
  try {
    const result = await invoke<OcrResult>("perform_ocr_capture");
    if (result) {
      // Create promise first so showImagePreview can detect it
      const ocrPromise = invoke<string>("ocr_image", { imageBase64: result.image_base64 });

      showImagePreview({
        base64: result.image_base64,
        mimeType: result.mime_type,
        ocrText: "[Processing...]",
        ocrPromise,
      });
      const index = state.attachedImages.length - 1;

      ocrPromise.then(text => {
        console.log("[OCR] Screenshot text:", text.substring(0, 50) + "...");
        if (state.attachedImages[index]) state.attachedImages[index].ocrText = text;
        // Remove loading indicator
        const container = document.getElementById("image-preview-container");
        const preview = container?.querySelector(`.image-preview[data-index="${index}"]`);
        if (preview) preview.classList.remove("ocr-processing");
      }).catch(err => {
        console.error("OCR failed:", err);
        if (state.attachedImages[index]) state.attachedImages[index].ocrText = "[OCR failed]";
        // Remove loading indicator
        const container = document.getElementById("image-preview-container");
        const preview = container?.querySelector(`.image-preview[data-index="${index}"]`);
        if (preview) preview.classList.remove("ocr-processing");
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
      // Reload chat history without page refresh to avoid flash
      chatArea.innerHTML = "";
      await loadChatHistory();
      await updateButtonStates();
    } catch (error) {
      console.error("Restore error:", error);
      const errorDiv = document.createElement("div");
      errorDiv.className = "message error-message";
      errorDiv.innerHTML = DOMPurify.sanitize(`
        <details class="error-accordion">
          <summary class="error-summary">Restore Error</summary>
          <div class="error-details"></div>
        </details>
      `);
      const detailsEl = errorDiv.querySelector('.error-details');
      if (detailsEl) detailsEl.textContent = String(error);
      chatArea.appendChild(errorDiv);
      chatArea.scrollTop = chatArea.scrollHeight;
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
      const errorDiv = document.createElement("div");
      errorDiv.className = "message error-message";
      errorDiv.innerHTML = DOMPurify.sanitize(`
        <details class="error-accordion">
          <summary class="error-summary">Delete Error</summary>
          <div class="error-details"></div>
        </details>
      `);
      const detailsEl = errorDiv.querySelector('.error-details');
      if (detailsEl) detailsEl.textContent = String(error);
      chatArea.appendChild(errorDiv);
      chatArea.scrollTop = chatArea.scrollHeight;
    }
  }
});

// Update button states on page load
updateButtonStates();

async function loadChatHistory() {
  const fragment = document.createDocumentFragment();
  try {
    const history = await invoke<ChatMessage[]>("get_chat_history");
    chatArea.innerHTML = ""; // Clear existing

    // Process messages sequentially
    for (const msg of history) {
      const displayRole = msg.is_cron ? "cron" : msg.role;

      if (displayRole === "user" || displayRole === "cron") {
        // Reset web search container for each user message (new turn)
        resetWebSearchContainer();
        // Pass all images if present in history
        addMessage(displayRole as "user" | "assistant" | "cron", msg.content || "", msg.images, fragment);
      } else if (displayRole === "assistant") {
        // 1. Render Reasoning (if present)
        if (msg.reasoning) {
          const thinkingMsg = createThinkingElement(msg.reasoning, true);
          fragment.appendChild(thinkingMsg);
        }

        // 2. Render Tool Calls (if present)
        if (msg.tool_calls && msg.tool_calls.length > 0) {
          msg.tool_calls.forEach((toolCall: any) => {
            const toolName = toolCall.function.name;

            if (isWebSearchTool(toolName)) {
              // Group web searches into container
              const container = getOrCreateWebSearchContainer(fragment);
              if (!fragment.contains(container)) {
                fragment.appendChild(container);
              }

              const queriesContainer = container.querySelector(".web-search-queries");
              if (queriesContainer) {
                let query = "";
                try {
                  const args = JSON.parse(toolCall.function.arguments);
                  query = args.query || "";
                } catch (e) {
                  query = toolCall.function.arguments;
                }
                const queryEl = createWebSearchQueryElement(query, toolCall.id);
                queriesContainer.appendChild(queryEl);
                updateWebSearchCount(container);
              }
            } else {
              const toolDiv = createToolCallElement(
                toolName,
                toolCall.function.arguments,
                toolCall.id,
                false,
              );
              fragment.appendChild(toolDiv);
            }
          });
        }

        // 3. Render Assistant Content (if present)
        if (msg.content) {
          addMessage("assistant", msg.content, msg.images, fragment);
        }
      } else if (msg.role === "tool") {
        // Tool result message - try to find matching element by ID
        let matched = false;

        // First, try to match web search query by ID
        if (msg.tool_call_id) {
          const webSearchQueries = Array.from(fragment.querySelectorAll(".web-search-query"));
          const matchingQuery = webSearchQueries.find((el) =>
            el.getAttribute("data-tool-id") === msg.tool_call_id
          );

          if (matchingQuery && msg.content) {
            const resultSection = matchingQuery.querySelector('.tool-result') as HTMLElement;
            const resultContent = matchingQuery.querySelector('.tool-result-content');
            if (resultSection && resultContent) {
              // Simplify web search results for display (extract just title links)
              const cleanResult = msg.content
                .replace(/^Web Search Results:\n/, '')
                // Extract markdown links and remove snippets after " : "
                .split('\n')
                .filter((line: string) => line.trim().startsWith('-'))
                .map((line: string) => {
                  // Match "- [title](url) : snippet" and keep just "- [title](url)"
                  const match = line.match(/^(- \[[^\]]+\]\([^)]+\))/);
                  return match ? match[1] : line;
                })
                .join('\n');
              resultContent.innerHTML = DOMPurify.sanitize(md.render(preprocessMarkdown(cleanResult)));
              resultSection.style.display = 'grid';
              matched = true;
            }
          }
        }

        // If not matched as web search, try regular tool-output
        if (!matched) {
          const toolMessages = Array.from(fragment.querySelectorAll(".tool-output"));
          let matchingTool: Element | undefined;

          // Try to match by ID first if available
          if (msg.tool_call_id) {
            matchingTool = toolMessages
              .reverse()
              .find((el) => el.getAttribute("data-tool-id") === msg.tool_call_id);
          }

          // Fallback to the last one if no ID match
          if (!matchingTool) {
            matchingTool = toolMessages[toolMessages.length - 1];
          }

          if (matchingTool && msg.content) {
            updateToolResult(matchingTool, msg.content);
          }
        }
      }
    }

    chatArea.appendChild(fragment);

    // Scroll to bottom
    chatArea.scrollTop = chatArea.scrollHeight;
  } catch (e) {
    console.error("Failed to load chat history:", e);
    if (fragment.hasChildNodes()) {
      chatArea.appendChild(fragment);
    }
  }
}

loadChatHistory();

// Listen for agent retry events (backend requesting UI clear before retry)
listen<string>(EVENTS.AGENT_RETRY, (event) => {
  try {
    const payload = JSON.parse(event.payload);
    console.log("[Agent Retry] Received retry event:", payload);

    // Clear the failed response from UI before retry
    // Remove elements in reverse order until we hit the user message
    while (chatArea.lastElementChild) {
      const el = chatArea.lastElementChild;
      // Stop if we hit a user message
      if (el.classList.contains("user")) break;

      // Remove assistant, tool, thinking, and web search elements
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

    // Reset state for new response
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
    console.error("[Agent Retry] Failed to parse retry event:", e);
  }
});

// Listen for background cron jobs starting
listen<string>("agent-cron-started", (event) => {
  const prompt = DOMPurify.sanitize(event.payload);
  resetWebSearchContainer();
  addMessage("cron", prompt, undefined);
});

// Listen for agent streaming response chunks
listen<string>(EVENTS.AGENT_RESPONSE_CHUNK, (event) => {
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

    // Create content wrapper for streaming updates
    const contentDiv = document.createElement("div");
    contentDiv.className = "message-content";
    msgDiv.appendChild(contentDiv);

    // Add copy button
    const copyBtn = document.createElement("button");
    copyBtn.className = "copy-btn";
    copyBtn.title = "Copy as Markdown";
    copyBtn.innerHTML = COPY_ICON;
    copyBtn.addEventListener("click", (e) => {
      e.stopPropagation();
      const raw = msgDiv.getAttribute("data-raw") || "";
      navigator.clipboard.writeText(raw).then(() => {
        const originalHTML = copyBtn.innerHTML;
        copyBtn.innerHTML = CHECK_ICON;
        copyBtn.classList.add("copied");
        setTimeout(() => {
          copyBtn.innerHTML = originalHTML;
          copyBtn.classList.remove("copied");
        }, 1500);
      }).catch((err) => console.error("Failed to copy:", err));
    });
    msgDiv.appendChild(copyBtn);

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
      updateThinkingElement(openThinking as HTMLElement, openThinking.getAttribute("data-thinking") || "", true);
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
            ${DOMPurify.sanitize(md.render(preprocessMarkdown(rest)))}
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
    // No thought tags, render normally with preprocessing for KaTeX
    html = DOMPurify.sanitize(md.render(preprocessMarkdown(rawText)));
  }

  // Update only the content div, not the full innerHTML (preserves copy button)
  const contentDiv = lastMsg.querySelector(".message-content");
  if (contentDiv) {
    contentDiv.innerHTML = html;
  } else {
  // Fallback for messages without content wrapper (shouldn't happen)
    lastMsg.innerHTML = html;
  }
  chatArea.scrollTop = chatArea.scrollHeight;
});

listen<string>(EVENTS.AGENT_REASONING_CHUNK, (event) => {
  // ============================================================================
  // REASONING CHUNK HANDLER
  // ============================================================================
  // Handles model reasoning/thinking output in collapsible blocks.
  //
  // Uses session-based tracking via `currentThinkingBlock`:
  // - All thoughts within a single response turn merge into ONE block
  // - Block stays CLOSED to prevent visual flashing during fast inference
  // - Reset when new response starts (in handleInput)
  // ============================================================================

  const content = event.payload;
  console.log("Received reasoning chunk:", content);

  // Use the session thinking block, or create one if needed
  if (!state.currentThinkingBlock || !chatArea.contains(state.currentThinkingBlock)) {
    state.currentThinkingBlock = createThinkingElement(content, false);
    chatArea.appendChild(state.currentThinkingBlock);
  } else {
    // Append content to the session block
    let thinkingText = state.currentThinkingBlock.getAttribute("data-thinking") || "";
    thinkingText += content;
    updateThinkingElement(state.currentThinkingBlock, thinkingText, false);
  }

  chatArea.scrollTop = chatArea.scrollHeight;

  chatArea.scrollTop = chatArea.scrollHeight;
});

listen<string>(EVENTS.AGENT_TOOL_CALL, (event) => {
  const payload = JSON.parse(event.payload);

  // Idempotent update: find existing block by ID
  let toolDiv = payload.id ? chatArea.querySelector(`[data-tool-id="${payload.id}"]`) as HTMLElement : null;

  if (toolDiv) {
    // Update existing tool call (e.g. arguments streaming)
    const summaryArgs = toolDiv.querySelector(".tool-summary-args");
    if (summaryArgs) {
      const argsText = Object.entries(payload.args || {})
        .map(([k, v]) => `${md.utils.escapeHtml(k)}="${md.utils.escapeHtml(String(v))}"`)
        .join(" ");
      summaryArgs.textContent = argsText;
    }
    const toolArgs = toolDiv.querySelector(".tool-args");
    if (toolArgs) {
      if (payload.rawArgs) {
        toolArgs.textContent = payload.rawArgs;
      } else {
        toolArgs.textContent = JSON.stringify(payload.args, null, 2);
      }
    }
    return;
  }

  // Complete any open thinking blocks before showing new tool call
  const openThinking = chatArea.querySelector('.thinking-output:not([data-complete="true"])');
  if (openThinking) {
    updateThinkingElement(openThinking as HTMLElement, openThinking.getAttribute("data-thinking") || "", true);
  }

  if (isWebSearchTool(payload.name)) {
    // Group web searches into a single container
    const container = getOrCreateWebSearchContainer(chatArea);

    // If this is the first web search, add the container to chat
    if (!chatArea.contains(container)) {
      chatArea.appendChild(container);
    }

    // Add the query to the container
    const queriesContainer = container.querySelector(".web-search-queries");
    if (queriesContainer) {
      const query = payload.args?.query || "";
      const queryEl = createWebSearchQueryElement(query, payload.id);
      queriesContainer.appendChild(queryEl);
      updateWebSearchCount(container);
    }
  } else {
    // Regular tool call - render as accordion
    const newToolDiv = createToolCallElement(payload.name, JSON.stringify(payload.args), payload.id);
    chatArea.appendChild(newToolDiv);
  }

  chatArea.scrollTop = chatArea.scrollHeight;
});

// Listen for tool results and add them to the matching tool call
listen<string>(EVENTS.AGENT_TOOL_RESULT, (event) => {
  const payload = JSON.parse(event.payload);
  const name = payload.name;
  const result = payload.result;

  if (isWebSearchTool(name)) {
    // Find matching web search query element
    const webSearchQueries = Array.from(chatArea.querySelectorAll(".web-search-query"));
    const matchingQuery = webSearchQueries
      .reverse()
      .find((el) => {
        const resultSection = el.querySelector('.tool-result') as HTMLElement;
        // Find the one that doesn't have a result yet
        return resultSection && resultSection.style.display === 'none';
      });

    if (matchingQuery) {
      const resultSection = matchingQuery.querySelector('.tool-result') as HTMLElement;
      const resultContent = matchingQuery.querySelector('.tool-result-content');
      if (resultSection && resultContent) {
        // Simplify web search results for display (extract just title links)
        const resultText = typeof result === "string" ? result : JSON.stringify(result, null, 2);
        // Remove "Web Search Results:" prefix and extract just the links
        const cleanResult = resultText
          .replace(/^Web Search Results:\n/, '')
          // Extract markdown links and remove snippets after " : "
          .split('\n')
          .filter((line: string) => line.trim().startsWith('-'))
          .map((line: string) => {
            // Match "- [title](url) : snippet" and keep just "- [title](url)"
            const match = line.match(/^(- \[[^\]]+\]\([^)]+\))/);
            return match ? match[1] : line;
          })
          .join('\n');
        resultContent.innerHTML = DOMPurify.sanitize(md.render(preprocessMarkdown(cleanResult)));
        resultSection.style.display = 'grid';
      }
    }
  } else {
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
  }
});

listen(EVENTS.AGENT_PROCESSING_START, () => {
  // Optional: Show a "thinking" indicator
});

// Listen for API errors and display with retry button
listen<string>(EVENTS.AGENT_ERROR, (event) => {
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
  // Only show the fallback message once per conversation turn
  if (state.fallbackShownThisTurn) {
    console.log("[Fallback] Skipping duplicate notification");
    return;
  }
  state.fallbackShownThisTurn = true;

  try {
    const data = JSON.parse(event.payload);
    const title = data.title || "Provider Fallback";
    const details = data.details || "";

    console.log("[Fallback]", title, details);

    // Create a non-blocking notification accordion in chat
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
    console.error("Failed to parse fallback event:", e);
  }
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
  }, 2000);
})();

// Window Visibility Logic
async function startHide() {
  const app = document.getElementById("app");
  if (app) {
    // Also hide settings modal if open
    const settingsModalEl = document.querySelector(".settings-modal");
    if (settingsModalEl) {
      settingsModalEl.classList.add("hidden");
    }
    app.classList.add("hidden-app");
    // Wait for transition to finish (200ms)
    setTimeout(async () => {
      await invoke("hide_window");
    }, 200);
  }
}

listen(EVENTS.START_HIDE, () => {
  startHide();
});

listen(EVENTS.START_SHOW, async () => {
  const app = document.getElementById("app");
  if (app) {
    // Small delay to ensure window is rendered before fading in
    setTimeout(() => {
      app.classList.remove("hidden-app");
      // Focus input
      inputField.focus();
    }, 50);
  }

  // Show loading chips if screen context is enabled (check config)
  try {
    const config = await invoke<{ enable_screen_context?: boolean; incognito_mode?: boolean }>("get_config");
    if (config.enable_screen_context && !config.incognito_mode) {
      showLoadingChips();
    }
  } catch (e) {
    console.warn("[ScreenContext] Failed to check config:", e);
  }
});

// Screen Context Suggestions
interface ScreenContext {
  capture_time_ms: number;
  suggestions: string[];
  context_summary: string;
  image_base64: string;
  mime_type: string;
  ocr_text: string;
}



// Create suggestions container (positioned above bottom-bar)
const suggestionsContainer = document.createElement("div");
suggestionsContainer.className = "suggestion-pills hidden";
suggestionsContainer.id = "suggestion-pills";
document.getElementById("app")?.querySelector(".bottom-bar")?.before(suggestionsContainer);

// Hide suggestions when user starts typing
inputField.addEventListener("input", () => {
  hideSuggestions();
});



function showSuggestions(suggestions: string[]) {
  if (suggestions.length === 0) return;

  state.currentSuggestions = suggestions; // Store for click handler

  // Generate short display names (max 30 chars) with full prompt in title
  suggestionsContainer.innerHTML = suggestions.map((s, i) => {
    const displayName = s.length > 30 ? s.substring(0, 27) + "..." : s;
    // Escape HTML for both display and title attribute
    const escapedDisplayName = md.utils.escapeHtml(displayName);
    const escapedTitle = md.utils.escapeHtml(s);
    return `<button class="suggestion-pill" data-index="${i}" title="${escapedTitle}">${escapedDisplayName}</button>`;
  }).join("");

  // Add click handlers
  suggestionsContainer.querySelectorAll(".suggestion-pill").forEach((pill) => {
    pill.addEventListener("click", (e) => {
      e.stopPropagation(); // Prevent click-to-hide
      const index = parseInt((pill as HTMLElement).dataset.index || "0", 10);
      inputField.value = state.currentSuggestions[index] || pill.textContent || "";

      // Attach the screen context image to chat (if available)
      if (state.currentScreenContextImage) {
        showImagePreview({
          base64: state.currentScreenContextImage.base64,
          mimeType: state.currentScreenContextImage.mimeType,
          ocrText: state.currentScreenContextImage.ocrText,
        });
      }

      inputField.focus();
      hideSuggestions();
      // Trigger input resize
      inputField.dispatchEvent(new Event("input"));
    });
  });

  suggestionsContainer.classList.remove("hidden");
  suggestionsContainer.classList.remove("loading");

  // Force reflow to ensure transition plays on first appearance
  void suggestionsContainer.offsetHeight;

  // Scroll chat to bottom so suggestions don't obscure messages
  setTimeout(() => {
    chatArea.scrollTo({ top: chatArea.scrollHeight, behavior: 'smooth' });
  }, 50);

  // Auto-hide after 15 seconds
  if (state.suggestionTimeout) clearTimeout(state.suggestionTimeout);
  state.suggestionTimeout = setTimeout(hideSuggestions, 15000);
}

function showLoadingChips() {
  suggestionsContainer.innerHTML = `
    <span class="suggestion-pill loading-pill">Analyzing screen...</span>
  `;
  // Start hidden, force reflow, then show to trigger animation
  suggestionsContainer.classList.add("hidden");
  void suggestionsContainer.offsetHeight; // Force reflow
  suggestionsContainer.classList.remove("hidden");
  suggestionsContainer.classList.add("loading");

  // Scroll chat to bottom after CSS transition completes (300ms)
  setTimeout(() => {
    chatArea.scrollTo({ top: chatArea.scrollHeight, behavior: 'smooth' });
  }, 200);
}

function hideSuggestions() {
  suggestionsContainer.classList.add("hidden");
  suggestionsContainer.classList.remove("loading");
  if (state.suggestionTimeout) {
    clearTimeout(state.suggestionTimeout);
    state.suggestionTimeout = null;
  }
}

listen<ScreenContext>(EVENTS.SCREEN_CONTEXT_READY, (event) => {
  console.log("[ScreenContext] Received suggestions:", event.payload);

  // Store image for when a suggestion is clicked
  state.currentScreenContextImage = {
    base64: event.payload.image_base64,
    mimeType: event.payload.mime_type,
    ocrText: event.payload.ocr_text
  };

  showSuggestions(event.payload.suggestions);
});

// Click-to-Hide Logic
document.addEventListener("click", (e) => {
  const target = e.target as HTMLElement;
  // Interactive elements: input container, messages, settings modal, bottom bar, buttons, image preview
  // We also check if the click was on a text selection? (Browser handles this, click fires after mouseup)
  // If user selects text, they might click? No, selection is drag. Click is click.

  const isInteractive = target.closest(
    ".input-container, .message, .settings-modal, .bottom-bar, .action-btn, .stop-btn, .image-preview, .suggestion-pills",
  );

  if (!isInteractive) {
    startHide();
  }
});

// Settings Modal Logic
const settingsModal = document.createElement("div");
settingsModal.className = "settings-modal hidden";
settingsModal.innerHTML = SETTINGS_MODAL_HTML;
document.body.appendChild(settingsModal);

// Tab switching logic
initSettingsTabs(settingsModal);

const geminiKeyInput = document.getElementById("gemini-key") as HTMLInputElement;
const openRouterKeyInput = document.getElementById("openrouter-key") as HTMLInputElement;
const cerebrasKeyInput = document.getElementById("cerebras-key") as HTMLInputElement;
const groqKeyInput = document.getElementById("groq-key") as HTMLInputElement;
const braveKeyInput = document.getElementById("brave-key") as HTMLInputElement;
const modelInput = document.getElementById("model-id") as HTMLSelectElement;
const backgroundModelInput = document.getElementById("background-model-id") as HTMLSelectElement;
const providerConflictWarning = document.getElementById("provider-conflict-warning") as HTMLDivElement;
const enableToolsCheckbox = document.getElementById("enable-tools") as HTMLInputElement;
const incognitoModeCheckbox = document.getElementById("incognito-mode") as HTMLInputElement;
const enableScreenContextCheckbox = document.getElementById("enable-screen-context") as HTMLInputElement;
const saveSettingsBtn = document.getElementById("save-settings") as HTMLButtonElement;
const closeSettingsBtn = document.getElementById("close-settings") as HTMLButtonElement;

// Define unsupported models (no tool calling support)
const UNSUPPORTED_TOOL_MODELS = [
  "allenai/olmo-3.1-32b-think:free"
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

// Provider conflict detection
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

modelInput.addEventListener("change", checkProviderConflict);
backgroundModelInput.addEventListener("change", checkProviderConflict);

// Model types from backend
interface ModelInfo {
  id: string;
  display_name: string;
  provider: "gemini" | "openrouter" | "groq" | "cerebras";
  category: "chat" | "vision" | "background";
  supports_tools: boolean;
}

interface ModelsResponse {
  chat_models: ModelInfo[];
  vision_models: ModelInfo[];
  background_models: ModelInfo[];
}

// Helper to populate a dropdown with models grouped by provider
function populateModelDropdown(
  selectEl: HTMLSelectElement,
  models: ModelInfo[],
  selectedValue: string | null
) {
  // Clear existing options
  selectEl.innerHTML = "";

  // Group models by provider display name
  const providerDisplayNames: Record<string, string> = {
    gemini: "Gemini AI",
    openrouter: "OpenRouter",
    groq: "Groq",
    cerebras: "Cerebras",
  };

  const groups: Record<string, ModelInfo[]> = {};
  for (const model of models) {
    const providerKey = model.provider;
    if (!groups[providerKey]) {
      groups[providerKey] = [];
    }
    groups[providerKey].push(model);
  }

  // Create optgroups in order: gemini, openrouter, groq, cerebras
  const providerOrder = ["gemini", "openrouter", "groq", "cerebras"];
  for (const provider of providerOrder) {
    const modelsInGroup = groups[provider];
    if (!modelsInGroup || modelsInGroup.length === 0) continue;

    const optgroup = document.createElement("optgroup");
    optgroup.label = providerDisplayNames[provider] || provider;

    for (const model of modelsInGroup) {
      const option = document.createElement("option");
      option.value = model.id;
      option.textContent = model.display_name;
      option.dataset.provider = model.provider;
      optgroup.appendChild(option);
    }

    selectEl.appendChild(optgroup);
  }

  // Set selected value if provided and exists
  if (selectedValue) {
    // Check if the value exists in options
    const exists = Array.from(selectEl.options).some(opt => opt.value === selectedValue);
    if (exists) {
      selectEl.value = selectedValue;
    }
  }
}

settingsBtn.addEventListener("click", async () => {
  try {
    // Load models from backend first
    const modelsResponse = await invoke<ModelsResponse>("get_available_models");

    // Load config
    const config = await invoke<any>("get_config");

    // Populate dropdowns with models from backend
    populateModelDropdown(
      modelInput,
      modelsResponse.chat_models,
      config.selected_model || "gemini-2.5-flash"
    );
    populateModelDropdown(
      backgroundModelInput,
      modelsResponse.background_models,
      config.background_model || "gpt-oss-120b (Groq)"
    );

    // Set other config values
    geminiKeyInput.value = config.gemini_api_key || "";
    openRouterKeyInput.value = config.openrouter_api_key || "";
    cerebrasKeyInput.value = config.cerebras_api_key || "";
    groqKeyInput.value = config.groq_api_key || "";
    braveKeyInput.value = config.brave_api_key || "";
    enableToolsCheckbox.checked = config.enable_tools || false;
    incognitoModeCheckbox.checked = config.incognito_mode || false;
    enableScreenContextCheckbox.checked = config.enable_screen_context || false;

    // Disable screen context when incognito mode is enabled
    enableScreenContextCheckbox.disabled = incognitoModeCheckbox.checked;

    updateToolAvailability(); // Run check on open
    checkProviderConflict(); // Check for provider conflicts

    settingsModal.classList.remove("hidden");
  } catch (e) {
    console.error("Failed to load config", e);
  }
});

closeSettingsBtn.addEventListener("click", () => {
  settingsModal.classList.add("hidden");
  inputField.focus();
});

saveSettingsBtn.addEventListener("click", async () => {
  const config = {
    gemini_api_key: geminiKeyInput.value || null,
    openrouter_api_key: openRouterKeyInput.value || null,
    cerebras_api_key: cerebrasKeyInput.value || null,
    groq_api_key: groqKeyInput.value || null,
    brave_api_key: braveKeyInput.value || null,
    selected_model: modelInput.value || null,
    background_model: backgroundModelInput.value || null,
    enable_web_search: true, // Default to true for now
    enable_tools: enableToolsCheckbox.checked,
    incognito_mode: incognitoModeCheckbox.checked,
    enable_screen_context: enableScreenContextCheckbox.checked,
  };

  try {
    await invoke("save_config", { config });
    alert("Settings saved!");
    settingsModal.classList.add("hidden");
    inputField.focus();
  } catch (e) {
    alert(`Failed to save settings: ${e}`);
  }
});

// Sessions Modal Logic
const sessionsBtn = document.getElementById("sessions-btn") as HTMLButtonElement;
const sessionsModal = document.createElement("div");
sessionsModal.className = "settings-modal hidden"; // reuse settings modal positioning
sessionsModal.innerHTML = SESSIONS_MODAL_HTML;
document.body.appendChild(sessionsModal);

const closeSessionsBtn = document.getElementById("close-sessions") as HTMLButtonElement;
const newChatBtn = document.getElementById("new-session-modal-btn") as HTMLButtonElement;
const sessionsListContainer = document.getElementById("sessions-list-container") as HTMLDivElement;

sessionsBtn.addEventListener("click", async () => {
  sessionsModal.classList.remove("hidden");

  sessionsListContainer.innerHTML = '<div class="loading-spinner">Loading sessions...</div>';
  try {
    const resultString = await invoke<string>("get_recent_sessions", { limit: 20 });
    if (resultString === "No matching sessions found.") {
      sessionsListContainer.innerHTML = '<div class="sessions-empty">No recent sessions found.</div>';
      return;
    }

    sessionsListContainer.innerHTML = '';
    const sessions = JSON.parse(resultString);
    sessions.forEach((s: any) => {
      const item = document.createElement("div");
      item.className = "session-item";
      item.dataset.id = s.session_id;

      const titleEl = document.createElement("div");
      titleEl.className = "session-item-title";
      titleEl.textContent = s.title;

      const metaEl = document.createElement("div");
      metaEl.className = "session-item-meta";

      const dateSpan = document.createElement("span");
      dateSpan.textContent = new Date(s.date).toLocaleDateString();

      const summarySpan = document.createElement("span");
      summarySpan.className = "session-item-summary";
      summarySpan.textContent = s.summary !== "No summary available" ? s.summary.substring(0, 120) + (s.summary.length > 120 ? "..." : "") : "";

      metaEl.appendChild(dateSpan);
      metaEl.appendChild(summarySpan);

      item.appendChild(titleEl);
      item.appendChild(metaEl);

      sessionsListContainer.appendChild(item);
    });


    // Add click listeners
    sessionsListContainer.querySelectorAll('.session-item').forEach(el => {
      el.addEventListener('click', async (e) => {
        const id = (e.currentTarget as HTMLElement).dataset.id;
        if (id) {
          sessionsModal.classList.add("hidden");
          await invoke("load_session", { sessionId: id });
          chatArea.innerHTML = "";
          await loadChatHistory();
          await updateButtonStates();
          inputField.focus();
        }
      });
    });

  } catch (e) {
    sessionsListContainer.innerHTML = `<div class="sessions-error">Failed to load sessions: <span class="sessions-error-details"></span></div>`;
    const detailsSpan = sessionsListContainer.querySelector('.sessions-error-details');
    if (detailsSpan) detailsSpan.textContent = String(e);
  }
});

closeSessionsBtn.addEventListener("click", () => {
  sessionsModal.classList.add("hidden");
  inputField.focus();
});

newChatBtn.addEventListener("click", async () => {
  sessionsModal.classList.add("hidden");
  await invoke("save_and_clear_chat");
  chatArea.innerHTML = "";
  await updateButtonStates();
  inputField.focus();
});

// Development-only benchmark helper
if ((import.meta as any).env.DEV) {
  (window as any).runImageBench = async () => {
    console.log("Loading benchmark module...");
    try {
      const { runBenchmark } = await import("./tests/image_bench");
      await runBenchmark(resizeImage);
    } catch (err) {
      console.error("Failed to run benchmark:", err);
    }
  };
  console.log("Development mode: runImageBench() available in console");
}
