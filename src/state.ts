/**
 * Centralized mutable state for the Shard chat frontend.
 *
 * Replaces the scattered module-level `let` variables that previously lived in
 * main.ts. Grouping them here makes mutation sites explicit, simplifies reset
 * logic between turns, and opens the door to future persistence or reactivity.
 */

import type { AttachedImage } from "./types";

/** Lightweight value object for the screen-context image cached between suggestion clicks. */
export interface ScreenContextImage {
  base64: string;
  mimeType: string;
  ocrText: string;
}

export class ChatState {
  // ── Chat processing ──────────────────────────────────────────────
  isProcessing = false;
  isCancelled = false;

  // ── Image attachments ────────────────────────────────────────────
  attachedImages: AttachedImage[] = [];
  lastAttachedImages: AttachedImage[] = [];

  // ── Resend / retry ───────────────────────────────────────────────
  lastUserMessage = "";

  // ── Per-turn flags ───────────────────────────────────────────────
  /** Prevents duplicate "Moving to OpenRouter" toasts within a single turn. */
  fallbackShownThisTurn = false;

  /** Session-scoped thinking block for merging reasoning chunks. */
  currentThinkingBlock: HTMLElement | null = null;

  // ── Screen-context suggestions ───────────────────────────────────
  currentScreenContextImage: ScreenContextImage | null = null;
  currentSuggestions: string[] = [];
  suggestionTimeout: ReturnType<typeof setTimeout> | null = null;

  /**
   * Reset the subset of state that should be cleared at the start of every
   * new assistant turn (called from handleInput).
   */
  resetForNewTurn(): void {
    this.fallbackShownThisTurn = false;
    this.currentThinkingBlock = null;
  }
}
