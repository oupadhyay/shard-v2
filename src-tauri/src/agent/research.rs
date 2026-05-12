//! Research-mode intent classification.
//!
//! Calls the pinned `gemini-3.1-flash-lite-preview` `generateContent`
//! endpoint with the system [`crate::prompts::INTENT_CLASSIFICATION_PROMPT`]
//! and the user's last message. The classifier responds with a short
//! "YES"/"NO"; we return `true` when the (uppercased, trimmed) output
//! contains "YES".
//!
//! `process_message` consults this when the user has not explicitly enabled
//! research mode in config, allowing the agent to extend its turn budget
//! for complex research-style queries.

use super::Agent;

impl<R: tauri::Runtime> Agent<R> {
    pub(crate) async fn classify_intent(
        &self,
        query: &str,
        api_key: &str,
    ) -> Result<bool, String> {
        let url = crate::endpoints::gemini_classify();

        let payload = serde_json::json!({
            "contents": [{
                "parts": [{
                    "text": format!("{}\n\nQuery: {}", crate::prompts::INTENT_CLASSIFICATION_PROMPT, query)
                }]
            }],
            "generationConfig": {
                "temperature": 0.0,
                "maxOutputTokens": 10
            }
        });

        let client = reqwest::Client::new();
        let res = client
            .post(url)
            .header("X-Goog-Api-Key", api_key)
            .json(&payload)
            .send()
            .await
            .map_err(|e| e.to_string())?;

        if !res.status().is_success() {
            return Err(format!("Intent classification failed: {}", res.status()));
        }

        let body: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;

        if let Some(candidates) = body.get("candidates").and_then(|c| c.as_array()) {
            if let Some(first) = candidates.first() {
                if let Some(content) = first.get("content") {
                    if let Some(parts) = content.get("parts").and_then(|p| p.as_array()) {
                        if let Some(text_part) = parts.first() {
                            if let Some(text) = text_part.get("text").and_then(|t| t.as_str()) {
                                return Ok(text.trim().to_uppercase().contains("YES"));
                            }
                        }
                    }
                }
            }
        }

        Ok(false)
    }
}
