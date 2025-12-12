use reqwest;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use log;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

/// Brave Search API response structures
#[derive(Debug, Deserialize)]
struct BraveSearchResponse {
    web: Option<BraveWebResults>,
}

#[derive(Debug, Deserialize)]
struct BraveWebResults {
    results: Vec<BraveResult>,
}

#[derive(Debug, Deserialize)]
struct BraveResult {
    title: String,
    url: String,
    description: Option<String>,
}

/// Perform web search using Brave Search API (primary) or DuckDuckGo fallback
/// If brave_api_key is provided, uses Brave Search first
pub async fn perform_web_search(query: &str, brave_api_key: Option<&str>) -> Result<Vec<SearchResult>, String> {
    log::info!("Performing Web Search for: {}", query);

    // Try Brave Search first if API key is provided
    if let Some(api_key) = brave_api_key {
        if !api_key.is_empty() {
            match perform_brave_search(query, api_key).await {
                Ok(results) if !results.is_empty() => return Ok(results),
                Ok(_) => log::warn!("Brave Search returned no results, trying DuckDuckGo fallback"),
                Err(e) => log::warn!("Brave Search failed: {}, trying DuckDuckGo fallback", e),
            }
        }
    }

    // Fallback to DuckDuckGo
    perform_duckduckgo_search(query).await
}

/// Brave Search API (free tier: 2000 queries/month, no payment info required)
/// Sign up at: https://brave.com/search/api/
async fn perform_brave_search(query: &str, api_key: &str) -> Result<Vec<SearchResult>, String> {
    log::info!("Using Brave Search API");

    let client = reqwest::Client::builder()
        .build()
        .map_err(|e| format!("Failed to build client: {}", e))?;

    let url = format!(
        "https://api.search.brave.com/res/v1/web/search?q={}&count=5",
        urlencoding::encode(query)
    );

    let response = client
        .get(&url)
        .header("Accept", "application/json")
        .header("X-Subscription-Token", api_key)
        .send()
        .await
        .map_err(|e| format!("Brave Search network error: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("Brave Search API error: {}", response.status()));
    }

    let brave_response: BraveSearchResponse = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse Brave response: {}", e))?;

    let results = brave_response
        .web
        .map(|w| {
            w.results
                .into_iter()
                .take(5)
                .map(|r| SearchResult {
                    title: r.title,
                    url: r.url,
                    snippet: r.description.unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(results)
}

/// DuckDuckGo HTML scraping fallback
async fn perform_duckduckgo_search(query: &str) -> Result<Vec<SearchResult>, String> {
    log::info!("Using DuckDuckGo HTML fallback");

    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .build()
        .map_err(|e| format!("Failed to build client: {}", e))?;

    let url = "https://html.duckduckgo.com/html/";
    let params = [("q", query)];

    let response = client
        .post(url)
        .form(&params)
        .send()
        .await
        .map_err(|e| format!("Web search network error: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("Web search API error: {}", response.status()));
    }

    let html_content = response
        .text()
        .await
        .map_err(|e| format!("Failed to read response text: {}", e))?;

    // Detect CAPTCHA/bot blocking
    if html_content.contains("anomaly-modal") ||
       html_content.contains("bots use DuckDuckGo") ||
       html_content.contains("challenge to confirm") {
        log::error!("DuckDuckGo is blocking requests with a CAPTCHA");
        return Err("Web search blocked by CAPTCHA. Please configure a Brave API Key in Settings for reliable search (free at brave.com/search/api).".to_string());
    }

    let document = Html::parse_document(&html_content);
    let result_selector = Selector::parse(".result").unwrap();
    let title_selector = Selector::parse(".result__a").unwrap();
    let snippet_selector = Selector::parse(".result__snippet").unwrap();

    let mut results = Vec::new();

    for element in document.select(&result_selector) {
        let title_element = element.select(&title_selector).next();
        let snippet_element = element.select(&snippet_selector).next();

        if let (Some(title_el), Some(snippet_el)) = (title_element, snippet_element) {
            let title = title_el.text().collect::<Vec<_>>().join("");
            let url = title_el.value().attr("href").unwrap_or_default().to_string();
            let snippet = snippet_el.text().collect::<Vec<_>>().join("");

            if !title.is_empty() && !url.is_empty() {
                results.push(SearchResult {
                    title: title.trim().to_string(),
                    url: url.trim().to_string(),
                    snippet: snippet.trim().to_string(),
                });
            }
        }

        if results.len() >= 5 {
            break;
        }
    }

    if results.is_empty() {
        log::warn!("No web search results found for '{}' - DuckDuckGo may be rate limiting", query);
        return Err(format!("No search results found for '{}'. DuckDuckGo may be rate limiting. Consider adding a Brave API Key in Settings.", query));
    }

    Ok(results)
}
