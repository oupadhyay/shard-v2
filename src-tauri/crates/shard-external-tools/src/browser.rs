use reqwest::Url;
use reqwest::{header, Client};
use std::time::Duration;

/// Fetch a URL, simulate a real browser to avoid 403s, and use Jina Reader API to extract
/// clean Markdown, which natively handles headless JS rendering for dynamic sites.
pub async fn read_url(client: &Client, url: &str) -> Result<String, String> {
    let mut headers = header::HeaderMap::new();
    headers.insert(
        header::USER_AGENT,
        header::HeaderValue::from_static("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/145.0.0.0 Safari/537.36"),
    );

    // To use Jina, we just prepend https://r.jina.ai/
    let jina_url = format!("https://r.jina.ai/{}", url);
    let parsed_url = Url::parse(&jina_url).map_err(|e| format!("Invalid URL: {}", e))?;

    log::info!("Reading URL via Jina Reader API: {}", jina_url);

    // Use a reasonable timeout
    let response = client
        .get(parsed_url)
        .headers(headers)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("Network error connecting to Jina Reader: {}", e))?;

    if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
        log::warn!("Jina Reader API rate limit hit (429 Too Many Requests)");
        return Err("Jina Reader API rate limit exceeded (Free quota is 20 RPM). Please wait and try again or configure a Jina API key.".to_string());
    }

    if !response.status().is_success() {
        return Err(format!("Jina API Error: {}", response.status()));
    }

    let markdown = response
        .text()
        .await
        .map_err(|e| format!("Failed to read response body: {}", e))?;

    Ok(markdown)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore] // Hits the live Jina Reader API (needs network + a Jina key; can 401/429)
    async fn test_read_url_wikipedia() {
        let client = Client::new();
        let result = read_url(&client, "https://en.wikipedia.org/wiki/Tauri").await;
        assert!(
            result.is_ok(),
            "Failed to read Wikipedia: {:?}",
            result.err()
        );
        let md = result.unwrap();
        assert!(md.contains("Tauri"));
    }
}
