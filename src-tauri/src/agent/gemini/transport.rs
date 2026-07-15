use super::types::InteractionsRequest;

#[derive(Debug, Clone)]
pub struct GeminiInteractionsTransportConfig {
    pub endpoint_url: String,
    pub auth_token: String,
    pub api_revision: &'static str,
}

#[derive(Debug, Clone)]
pub struct GeminiGenerateContentTransportConfig {
    pub endpoint_url: String,
    pub auth_token: String,
}

pub async fn send_interactions_stream(
    client: &reqwest::Client,
    config: &GeminiInteractionsTransportConfig,
    request: &InteractionsRequest,
) -> Result<reqwest::Response, String> {
    client
        .post(format!("{}?alt=sse", config.endpoint_url))
        .header("x-goog-api-key", &config.auth_token)
        .header("Content-Type", "application/json")
        .header("Api-Revision", config.api_revision)
        .json(request)
        .send()
        .await
        .map_err(|e| format!("API network error: {}", e))
}

pub async fn send_interactions_request(
    client: &reqwest::Client,
    config: &GeminiInteractionsTransportConfig,
    request: &InteractionsRequest,
) -> Result<reqwest::Response, String> {
    client
        .post(&config.endpoint_url)
        .header("x-goog-api-key", &config.auth_token)
        .header("Content-Type", "application/json")
        .header("Api-Revision", config.api_revision)
        .json(request)
        .send()
        .await
        .map_err(|e| format!("API network error: {}", e))
}

pub async fn send_generate_content_request<B: serde::Serialize + ?Sized>(
    client: &reqwest::Client,
    config: &GeminiGenerateContentTransportConfig,
    request: &B,
) -> Result<reqwest::Response, String> {
    client
        .post(&config.endpoint_url)
        .header("X-Goog-Api-Key", &config.auth_token)
        .header("Content-Type", "application/json")
        .json(request)
        .send()
        .await
        .map_err(|e| format!("API network error: {}", e))
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::agent::gemini::{InteractionsGenerationConfig, GEMINI_API_REVISION};

    #[tokio::test]
    async fn send_interactions_stream_uses_configured_url_query_and_headers() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1beta/interactions"))
            .and(query_param("alt", "sse"))
            .and(header("x-goog-api-key", "test-gemini"))
            .and(header("content-type", "application/json"))
            .and(header("api-revision", GEMINI_API_REVISION))
            .respond_with(ResponseTemplate::new(200).set_body_string("data: [DONE]\n\n"))
            .expect(1)
            .mount(&server)
            .await;

        let config = GeminiInteractionsTransportConfig {
            endpoint_url: format!("{}/v1beta/interactions", server.uri()),
            auth_token: "test-gemini".to_string(),
            api_revision: GEMINI_API_REVISION,
        };
        let request = InteractionsRequest {
            model: "gemini-test".to_string(),
            input: json!([]),
            system_instruction: None,
            tools: None,
            generation_config: Some(InteractionsGenerationConfig {
                thinking_level: Some("low".to_string()),
                thinking_summaries: None,
                temperature: None,
                max_output_tokens: None,
            }),
            stream: true,
            store: Some(false),
        };

        let response = send_interactions_stream(&reqwest::Client::new(), &config, &request)
            .await
            .expect("request should succeed");

        assert!(response.status().is_success());
    }
}
