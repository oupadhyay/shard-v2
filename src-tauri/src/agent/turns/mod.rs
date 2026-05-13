//! Per-turn streaming handlers for the chat-completion loop.
//!
//! Each provider has its own module:
//!   - [`gemini`]: Google Generative Language *Interactions* API (SSE).
//!   - [`openrouter`]: OpenAI-compatible streaming used by OpenRouter, Groq,
//!     and Cerebras (Groq is identified inside the handler so it can do the
//!     quota → OpenRouter fallback).
//!
//! Both handlers consume a `&mut Vec<ChatMessage>` and append:
//!   - exactly one `assistant`/`model` message (with optional `tool_calls`)
//!   - zero or more `tool` rows (one per executed tool call)
//!
//! They return `Ok(true)` when at least one tool was invoked (so the outer
//! `process_message` loop must run another turn for the model to consume the
//! tool results) and `Ok(false)` otherwise.

mod gemini;
mod openrouter;
