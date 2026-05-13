//! Centralized network endpoint configuration.
//!
//! Production code reads each URL via the helpers in this module; defaults
//! match the historic hard-coded constants exactly so behaviour is unchanged.
//!
//! Tests can call [`set_overrides`] (gated behind `#[cfg(test)]`) to point
//! every endpoint at a `wiremock::MockServer` instance, allowing the agent
//! and its helpers to be exercised end-to-end without any real network IO.
//!
//! Adding a new endpoint? Two steps:
//!   1. Add the field to [`Endpoints`] with its production default.
//!   2. Add a thin getter (`pub fn my_endpoint() -> String`) and use it from
//!      the call site instead of inlining the URL string.

use std::sync::RwLock;

/// All overridable URLs in one struct. Each field is the *full URL* the
/// production code would have used as a literal.
#[derive(Clone, Debug)]
pub struct Endpoints {
    /// Gemini Interactions API (streaming chat turns).
    pub gemini_interactions: String,
    /// Gemini generateContent endpoint used by `Agent::classify_intent`
    /// (model is baked into the URL because the classifier is pinned to
    /// `gemini-3.1-flash-lite-preview`).
    pub gemini_classify: String,
    /// Base URL prefix for `DELETE` on uploaded Gemini files. The file name
    /// is appended at call time. Trailing slash NOT included.
    pub gemini_files_base: String,
    /// Initial resumable-upload URL for Gemini Files API.
    pub gemini_files_upload: String,
    /// Gemini text/multimodal embedding URL.
    pub gemini_embedding: String,
    /// OpenRouter chat completions URL (used as Groq quota fallback and by
    /// the Vision LLM helper for OpenRouter providers).
    pub openrouter_chat: String,
    /// Groq chat completions URL (used by Vision LLM helper).
    pub groq_chat: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            gemini_interactions: "https://generativelanguage.googleapis.com/v1beta/interactions".to_string(),
            gemini_classify: "https://generativelanguage.googleapis.com/v1beta/models/gemini-3.1-flash-lite-preview:generateContent".to_string(),
            gemini_files_base: "https://generativelanguage.googleapis.com/v1beta/files".to_string(),
            gemini_files_upload: "https://generativelanguage.googleapis.com/upload/v1beta/files".to_string(),
            gemini_embedding: "https://generativelanguage.googleapis.com/v1beta/models/gemini-embedding-2-preview:embedContent".to_string(),
            openrouter_chat: "https://openrouter.ai/api/v1/chat/completions".to_string(),
            groq_chat: "https://api.groq.com/openai/v1/chat/completions".to_string(),
        }
    }
}

static OVERRIDES: RwLock<Option<Endpoints>> = RwLock::new(None);

fn read<F: FnOnce(&Endpoints) -> String>(f: F) -> String {
    if let Ok(guard) = OVERRIDES.read() {
        if let Some(ref ep) = *guard {
            return f(ep);
        }
    }
    f(&Endpoints::default())
}

#[inline]
pub fn gemini_interactions() -> String {
    read(|e| e.gemini_interactions.clone())
}

#[inline]
pub fn gemini_classify() -> String {
    read(|e| e.gemini_classify.clone())
}

#[inline]
pub fn gemini_files_base() -> String {
    read(|e| e.gemini_files_base.clone())
}

#[inline]
pub fn gemini_files_upload() -> String {
    read(|e| e.gemini_files_upload.clone())
}

#[inline]
pub fn gemini_embedding() -> String {
    read(|e| e.gemini_embedding.clone())
}

#[inline]
pub fn openrouter_chat() -> String {
    read(|e| e.openrouter_chat.clone())
}

#[inline]
pub fn groq_chat() -> String {
    read(|e| e.groq_chat.clone())
}

/// Install a set of endpoint overrides. Subsequent calls to the getters
/// above will return the overridden values until [`clear_overrides`] is
/// called or another `set_overrides` replaces them.
///
/// Test-only: production code never installs overrides.
#[cfg(any(test, feature = "eval"))]
pub fn set_overrides(eps: Endpoints) {
    if let Ok(mut guard) = OVERRIDES.write() {
        *guard = Some(eps);
    }
}

/// Remove any test override and revert getters to production defaults.
#[cfg(any(test, feature = "eval"))]
pub fn clear_overrides() {
    if let Ok(mut guard) = OVERRIDES.write() {
        *guard = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Serialize tests in this module; they all touch the global override slot.
    static SERIAL: Mutex<()> = Mutex::new(());

    /// Defaults must match the production constants byte-for-byte.
    /// This is the parity guarantee for Phase 0.
    #[test]
    fn defaults_match_production_constants() {
        let _g = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
        clear_overrides();
        assert_eq!(
            gemini_interactions(),
            "https://generativelanguage.googleapis.com/v1beta/interactions"
        );
        assert_eq!(
            gemini_classify(),
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-3.1-flash-lite-preview:generateContent"
        );
        assert_eq!(
            gemini_files_base(),
            "https://generativelanguage.googleapis.com/v1beta/files"
        );
        assert_eq!(
            gemini_files_upload(),
            "https://generativelanguage.googleapis.com/upload/v1beta/files"
        );
        assert_eq!(
            gemini_embedding(),
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-embedding-2-preview:embedContent"
        );
        assert_eq!(
            openrouter_chat(),
            "https://openrouter.ai/api/v1/chat/completions"
        );
        assert_eq!(
            groq_chat(),
            "https://api.groq.com/openai/v1/chat/completions"
        );
    }

    #[test]
    fn override_then_clear_round_trip() {
        let _g = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
        let eps = Endpoints {
            gemini_interactions: "http://localhost:1/interactions".to_string(),
            ..Endpoints::default()
        };
        set_overrides(eps);
        assert_eq!(gemini_interactions(), "http://localhost:1/interactions");
        // Other fields fall through to defaults.
        assert_eq!(
            openrouter_chat(),
            "https://openrouter.ai/api/v1/chat/completions"
        );

        clear_overrides();
        assert_eq!(
            gemini_interactions(),
            "https://generativelanguage.googleapis.com/v1beta/interactions"
        );
    }
}
