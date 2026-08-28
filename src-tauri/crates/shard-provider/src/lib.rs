//! Host-free provider transports and wire contracts for Shard.
//!
//! The Shard host owns provider/model selection, credential and endpoint
//! lookup, retries, durable state, UI events, and workflow policy. This crate
//! accepts explicit transport configuration and request data.

pub mod chat;
pub mod gemini_chat;
pub mod gemini_embedding;
pub mod gemini_files;

pub use gemini_embedding::{
    generate_embedding, generate_multimodal_embedding, GeminiEmbeddingConfig,
};
pub use gemini_files::{
    delete_uploaded_gemini_file, upload_image_to_gemini_files_api, GeminiFileUri,
    GeminiFilesDeleteConfig, GeminiFilesUploadConfig,
};
