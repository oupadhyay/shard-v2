//! Compatibility exports for Gemini Files API transport.
//!
//! Transport and wire contracts live in `shard-provider`; the Shard host owns
//! upload routing, chat-history URI persistence, and cleanup policy.

pub use shard_provider::gemini_files::{
    delete_uploaded_gemini_file, upload_image_to_gemini_files_api, GeminiFilesDeleteConfig,
    GeminiFilesUploadConfig,
};
