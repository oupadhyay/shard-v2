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
  ocrPromise?: Promise<string>;
  previewUrl?: string;
}

// Chat message from backend history
export interface ChatMessage {
  role: string;
  content: string;
  is_cron?: boolean | null;
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
  cerebras_api_key?: string;
  groq_api_key?: string;
  brave_api_key?: string;
  selected_model?: string;
  background_model?: string;
  enable_web_search?: boolean;
  enable_tools?: boolean;
  incognito_mode?: boolean;
  research_mode?: boolean;
  enable_screen_context?: boolean;
  heartbeat_global_cooldown_secs?: number;
}

// Payload for chat command
export interface ChatMessagePayload {
  message: string;
  imagesBase64?: string[];
  imagesMimeTypes?: string[];
  [key: string]: unknown; // Index signature for Tauri invoke compatibility
}

// Model types from backend
export interface ModelInfo {
  id: string;
  display_name: string;
  provider: "gemini" | "openrouter" | "groq" | "cerebras";
  category: "chat" | "vision" | "background";
  supports_tools: boolean;
  supports_vision: boolean;
}

export interface ModelsResponse {
  chat_models: ModelInfo[];
  vision_models: ModelInfo[];
  background_models: ModelInfo[];
}

// Proactive message from the heartbeat engine
export interface ProactiveMessage {
  id: string;
  heartbeat_session: string;
  content: string;
  draft_payload?: string | null;
  needs_approval: boolean;
  reviewed_at?: string | null;
  approved?: boolean | null;
  created_at: string;
}

// Heartbeat spec status for the dashboard
export interface HeartbeatStatusInfo {
  filename: string;
  schedule: string;
  session: string;
  persona: string | null;
  max_tool_calls: number;
  max_runs_per_day: number | null;
  prompt_preview: string;
}
