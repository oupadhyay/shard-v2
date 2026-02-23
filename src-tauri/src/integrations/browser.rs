use reqwest::{header, Client};
use std::io::Cursor;
use reqwest::Url;
use std::time::Duration;

/// Fetch a URL, simulate a real browser to avoid 403s, and use readability to extract the core article body.
pub async fn read_url(client: &Client, url: &str) -> Result<String, String> {
    let mut headers = header::HeaderMap::new();
    // Simulate real browser as requested to bypass simple anti-bot mechanisms
    headers.insert(
        header::USER_AGENT,
        header::HeaderValue::from_static("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/145.0.0.0 Safari/537.36"),
    );
    headers.insert(
        header::ACCEPT,
        header::HeaderValue::from_static("text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8"),
    );
    headers.insert(
        header::ACCEPT_LANGUAGE,
        header::HeaderValue::from_static("en-US,en;q=0.5"),
    );

    let parsed_url = Url::parse(url).map_err(|e| format!("Invalid URL: {}", e))?;

    // Use a timeout to ensure we don't hang indefinitely on slow sites
    let response = client
        .get(parsed_url.clone())
        .headers(headers)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| format!("Network error: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("HTTP Error: {}", response.status()));
    }

    let html = response
        .text()
        .await
        .map_err(|e| format!("Failed to read response body: {}", e))?;

    // The readability crate extracts the "meaningful" article content from HTML
    let mut cursor = Cursor::new(html);
    let extracted = readability::extractor::extract(&mut cursor, &parsed_url)
        .map_err(|e| format!("Failed to extract article content: {:?}", e))?;

    // Use htmd to convert the deeply nested (but clean) HTML into Markdown
    let converter = htmd::HtmlToMarkdown::builder().build();
    let markdown = converter
        .convert(&extracted.content)
        .map_err(|e| format!("Failed to convert HTML to markdown: {:?}", e))?;

    // Return the title and markdown joined
    Ok(format!("# {}\n\n{}", extracted.title, markdown.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_read_url_wikipedia() {
        let client = Client::new();
        let result = read_url(&client, "https://en.wikipedia.org/wiki/Tauri").await;
        assert!(result.is_ok(), "Failed to read Wikipedia: {:?}", result.err());
        let md = result.unwrap();
        assert!(md.contains("Tauri"));
    }
}
