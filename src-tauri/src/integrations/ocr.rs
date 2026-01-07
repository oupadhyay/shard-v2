//! OCR module - Image text recognition via Vision LLM
//!
//! This module previously used Tesseract/Leptonica for OCR.
//! Now OCR is handled via `vision_llm::describe_image` which uses
//! Groq or OpenRouter Vision models for better multilingual support
//! and the ability to understand images without text.
//!
//! The `perform_ocr_capture` and `ocr_image` commands in lib.rs
//! now call vision_llm directly.

// This file is kept for reference. All OCR functionality has moved to:
// - integrations/vision_llm.rs - Vision LLM API calls
// - lib.rs - perform_ocr_capture and ocr_image commands
