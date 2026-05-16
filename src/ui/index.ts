/**
 * UI module barrel exports
 */
export { md, clearKatexErrors, getKatexErrors, hasKatexErrors, detectUnrenderedLatex, preprocessMarkdown } from "./markdown";
export {
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
} from "./messages";
export { RESEND_ICON, STOP_ICON, TRASH_ICON, UNDO_ICON, RETRY_ICON, COPY_ICON, CHECK_ICON } from "./icons";
export { SETTINGS_MODAL_HTML, initSettingsTabs, populateModelDropdown, populateHeartbeatsPanel } from "./settings";
export { SESSIONS_MODAL_HTML } from "./sessions";
export { resizeImage } from "./image";
export { formatSessionDate, logger } from "./utils";
export { mountDiffViewer } from "./diff-viewer";
export type { EditOutcome, DiffViewerController } from "./diff-viewer";
