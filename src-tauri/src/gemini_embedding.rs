//! Compatibility exports for Gemini embedding transport.
//!
//! Request/response DTOs and HTTP transport live in `shard-provider`; Shard
//! owns chunking, invalidation, vector persistence, retrieval, and policy.

pub use shard_provider::gemini_embedding::{
    generate_embedding, generate_multimodal_embedding, GeminiEmbeddingConfig,
};
