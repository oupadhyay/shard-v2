//! Stateless Gemini embedding client helpers.
//!
//! The 5-tier memory system decides when embeddings are needed and passes an
//! explicit endpoint/auth config here. This keeps memory ownership in the host
//! repo while making Gemini embedding transport extractable with other
//! provider API code.

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
struct EmbeddingRequest {
    content: EmbeddingContent,
    #[serde(rename = "outputDimensionality")]
    output_dimensionality: Option<u32>,
}

#[derive(Serialize, Deserialize, Debug)]
struct EmbeddingContent {
    parts: Vec<EmbeddingPart>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(untagged)]
enum EmbeddingPart {
    Text { text: String },
    InlineData { inline_data: InlineData },
}

#[derive(Serialize, Deserialize, Debug)]
struct InlineData {
    mime_type: String,
    data: String,
}

#[derive(Deserialize, Debug)]
struct EmbeddingResponse {
    embedding: EmbeddingValues,
}

#[derive(Deserialize, Debug)]
struct EmbeddingValues {
    values: Vec<f32>,
}

#[derive(Debug, Clone)]
pub struct GeminiEmbeddingConfig {
    pub endpoint_url: String,
    pub auth_token: String,
    pub output_dimensionality: Option<u32>,
}

pub async fn generate_embedding(
    client: &reqwest::Client,
    text: &str,
    config: &GeminiEmbeddingConfig,
) -> Result<Vec<f32>, String> {
    let payload = EmbeddingRequest {
        content: EmbeddingContent {
            parts: vec![EmbeddingPart::Text {
                text: text.to_string(),
            }],
        },
        output_dimensionality: config.output_dimensionality,
    };

    send_embedding_request(client, config, &payload).await
}

/// Generate a multimodal embedding from text + images using Gemini embeddings.
/// Images are passed as base64-encoded inline data alongside the text.
pub async fn generate_multimodal_embedding(
    client: &reqwest::Client,
    text: &str,
    images_base64: &[String],
    images_mime_types: &[String],
    config: &GeminiEmbeddingConfig,
) -> Result<Vec<f32>, String> {
    let mut parts = Vec::with_capacity(1 + images_base64.len());

    parts.push(EmbeddingPart::Text {
        text: text.to_string(),
    });

    for (data, mime) in images_base64.iter().zip(images_mime_types.iter()) {
        parts.push(EmbeddingPart::InlineData {
            inline_data: InlineData {
                mime_type: mime.clone(),
                data: data.clone(),
            },
        });
    }

    let payload = EmbeddingRequest {
        content: EmbeddingContent { parts },
        output_dimensionality: config.output_dimensionality,
    };

    send_embedding_request(client, config, &payload).await
}

async fn send_embedding_request(
    client: &reqwest::Client,
    config: &GeminiEmbeddingConfig,
    payload: &EmbeddingRequest,
) -> Result<Vec<f32>, String> {
    let res = client
        .post(&config.endpoint_url)
        .header("X-Goog-Api-Key", &config.auth_token)
        .json(payload)
        .send()
        .await
        .map_err(|e| format!("Embedding API network error: {}", e))?;

    if !res.status().is_success() {
        let error_text = res.text().await.unwrap_or_default();
        return Err(format!("Embedding API error: {}", error_text));
    }

    let body: EmbeddingResponse = res
        .json()
        .await
        .map_err(|e| format!("Failed to parse embedding response: {}", e))?;

    Ok(body.embedding.values)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn text_embedding_request_wire_shape_is_stable() {
        let request = EmbeddingRequest {
            content: EmbeddingContent {
                parts: vec![EmbeddingPart::Text {
                    text: "remember this".to_string(),
                }],
            },
            output_dimensionality: Some(768),
        };

        assert_eq!(
            serde_json::to_value(request).unwrap(),
            json!({
                "content": {
                    "parts": [{ "text": "remember this" }]
                },
                "outputDimensionality": 768
            })
        );
    }

    #[test]
    fn multimodal_embedding_request_wire_shape_is_stable() {
        let request = EmbeddingRequest {
            content: EmbeddingContent {
                parts: vec![
                    EmbeddingPart::Text {
                        text: "a diagram".to_string(),
                    },
                    EmbeddingPart::InlineData {
                        inline_data: InlineData {
                            mime_type: "image/png".to_string(),
                            data: "aW1hZ2U=".to_string(),
                        },
                    },
                ],
            },
            output_dimensionality: None,
        };

        assert_eq!(
            serde_json::to_value(request).unwrap(),
            json!({
                "content": {
                    "parts": [
                        { "text": "a diagram" },
                        {
                            "inline_data": {
                                "mime_type": "image/png",
                                "data": "aW1hZ2U="
                            }
                        }
                    ]
                },
                "outputDimensionality": null
            })
        );
    }

    #[test]
    fn embedding_response_wire_shape_is_stable() {
        let response: EmbeddingResponse = serde_json::from_value(json!({
            "embedding": { "values": [0.25, -0.5, 1.0] }
        }))
        .unwrap();

        assert_eq!(response.embedding.values, vec![0.25, -0.5, 1.0]);
    }
}
