/**
 * ChatController — Shared chat turn lifecycle controller.
 *
 * Encapsulates the full input → chat → post-process pipeline so both the
 * ambient window (main.ts) and the dedicated window (dedicated.ts) share
 * identical orchestration logic. Window-specific behavior is injected via
 * the `ChatHooks` interface.
 */

import type { AttachedImage, ImageAttachment, ChatMessagePayload } from "./types";
import type { ChatState } from "./state";
import {
  clearKatexErrors,
  getKatexErrors,
  detectUnrenderedLatex,
  resetWebSearchContainer,
  addMessage,
  logger,
} from "./ui";
import { invoke } from "@tauri-apps/api/core";

// ── Public Interfaces ─────────────────────────────────────────────────

/** DOM element references injected by each window entry point. */
export interface ChatDOMRefs {
  chatArea: HTMLDivElement;
  inputField: HTMLTextAreaElement;
  stopBtn: HTMLButtonElement;
  /** e.g. "image-preview-container" or "dedicated-image-preview-container" */
  imagePreviewContainerId: string;
}

/** Optional hooks for window-specific side effects. */
export interface ChatHooks {
  /** Called after a user message is rendered (e.g., updateNewChatButtonState). */
  onUserMessageRendered?: () => void;
  /** Called after a turn completes (success or error), in finally. */
  onTurnComplete?: () => void;
  /** Called after button states need refreshing (post-turn). */
  onUpdateButtonStates?: () => void;
}

/** Icon HTML strings for stop and resend button states. */
export interface ChatIcons {
  stop: string;
  resend: string;
}

// ── Controller ────────────────────────────────────────────────────────

export class ChatController {
  constructor(
    private dom: ChatDOMRefs,
    private state: ChatState,
    private hooks: ChatHooks,
    private icons: ChatIcons,
  ) {}

  // ── Public API ────────────────────────────────────────────────────

  /**
   * Full chat turn: guard → capture input → show processing UI →
   * invoke backend → post-process (thinking finalization, KaTeX check).
   *
   * @param skipUi - When true, skips rendering the user message bubble
   *                 (used by resend/retry flows that already have the
   *                 message in the DOM).
   */
  async handleInput(skipUi = false): Promise<void> {
    const text = this.dom.inputField.value.trim();
    if ((!text && !skipUi) || this.state.isProcessing) return;

    this.resetTurnState();
    const payload = this.captureInput(text, skipUi);
    this.enterProcessingState();

    try {
      await this.executeChatTurn(payload);
    } catch (error) {
      logger.error("Chat error:", error);
    } finally {
      this.exitProcessingState();
      await this.postProcess();
      this.hooks.onTurnComplete?.();
      this.hooks.onUpdateButtonStates?.();
    }
  }

  // ── Private: Turn Lifecycle ───────────────────────────────────────

  /** Clear per-turn flags so the new turn starts clean. */
  private resetTurnState(): void {
    this.state.resetForNewTurn();
    this.state.isCancelled = false;
  }

  /**
   * Snapshot attached images, update state for resend, render the user
   * message (unless skipUi), and build the chat payload.
   */
  private captureInput(text: string, skipUi: boolean): ChatMessagePayload {
    const currentImages = [...this.state.attachedImages];

    if (!skipUi) {
      this.state.lastUserMessage = text;
      this.state.lastAttachedImages = [...currentImages];

      addMessage(this.dom.chatArea, "user", text, currentImages);
      this.hooks.onUserMessageRendered?.();

      // Clear attachment state & DOM
      this.state.attachedImages = [];
      const container = document.getElementById(this.dom.imagePreviewContainerId);
      if (container) container.innerHTML = "";
    }

    // Clear input field (common to both paths)
    this.dom.inputField.value = "";
    this.dom.inputField.style.height = "auto";

    const finalImages: (AttachedImage | ImageAttachment)[] = skipUi
      ? currentImages
      : this.state.lastAttachedImages;
    const message = skipUi ? this.state.lastUserMessage : text;

    const payload: ChatMessagePayload = { message };
    if (finalImages.length > 0) {
      payload.imagesBase64 = finalImages.map((img) => img.base64);
      payload.imagesMimeTypes = finalImages.map((img) => img.mimeType);
    }

    return payload;
  }

  /** Show the stop button and reset web-search / KaTeX state. */
  private enterProcessingState(): void {
    this.state.isProcessing = true;
    resetWebSearchContainer();
    clearKatexErrors();

    this.dom.stopBtn.style.display = "inline-flex";
    this.dom.stopBtn.classList.add("loading");
    this.dom.stopBtn.innerHTML = this.icons.stop;
    this.dom.stopBtn.dataset.mode = "stop";
  }

  /** Send the payload to the Rust backend via Tauri IPC. */
  private async executeChatTurn(payload: ChatMessagePayload): Promise<void> {
    logger.info("Sending payload to backend:", {
      message: payload.message,
      hasImage: !!payload.imagesBase64,
      imageLen: payload.imagesBase64?.length,
      mime: payload.imagesMimeTypes,
    });
    await invoke("chat", payload);
  }

  /**
   * Restore the stop button, clear the processing flag, and finalize
   * any open thinking/reasoning block.
   */
  private exitProcessingState(): void {
    this.state.isProcessing = false;
    this.dom.stopBtn.classList.remove("loading");

    if (!this.state.isCancelled) {
      this.dom.stopBtn.style.display = "none";
    }

    // Finalize any remaining open thinking block
    const openThinking = this.dom.chatArea.querySelector(
      '.thinking-output:not([data-complete="true"])',
    );
    if (openThinking) {
      openThinking.setAttribute("data-complete", "true");
      const summary = openThinking.querySelector("summary");
      if (summary) summary.textContent = "Thought";
    }
  }

  /**
   * Post-turn quality gate: detect KaTeX parse errors and unrendered
   * LaTeX, then request a backend retry if issues are found.
   */
  private async postProcess(): Promise<void> {
    if (this.state.isCancelled) return;

    const parseErrors = getKatexErrors();

    const allMessages = this.dom.chatArea.querySelectorAll(
      ".message.assistant:not(.tool-output):not(.thinking-output)",
    );
    const lastAssistant =
      allMessages.length > 0 ? allMessages[allMessages.length - 1] : null;
    const responseText = lastAssistant?.getAttribute("data-raw") || "";

    logger.debug("[KaTeX Check] Raw response text:", responseText.slice(0, 200));
    const unrenderedErrors = detectUnrenderedLatex(responseText);
    const allErrors = [...parseErrors, ...unrenderedErrors];

    if (allErrors.length > 0) {
      logger.info("[KaTeX] Detected rendering issues, requesting retry:", allErrors);
      try {
        await invoke("retry_with_katex_hint", { katexErrors: allErrors });
      } catch (e) {
        logger.error("[KaTeX] Retry request failed:", e);
      }
    }
  }
}
