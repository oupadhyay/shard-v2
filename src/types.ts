/**
 * Shared TypeScript types for Shard
 */

// Image attachment in chat messages
export interface ImageAttachment {
  base64: string;
  mimeType: string;
}

// Attached image with OCR text (for input state)
export interface AttachedImage extends ImageAttachment {
  ocrText: string;
}

// Chat message from backend history
export interface ChatMessage {
  role: string;
  content: string;
  images?: ImageAttachment[];
  tool_calls?: ToolCall[];
  tool_call_id?: string | null;
  reasoning?: string | null;
}

// Tool call structure
export interface ToolCall {
  id: string;
  function: {
    name: string;
    arguments: string;
  };
}

// OCR capture result from Tauri
export interface OcrResult {
  text: string;
  image_base64: string;
  mime_type: string;
}

// App configuration from backend
export interface AppConfig {
  gemini_api_key?: string;
  openrouter_api_key?: string;
  selected_model?: string;
  enable_web_search?: boolean;
  enable_tools?: boolean;
  incognito_mode?: boolean;
  research_mode?: boolean;
}
