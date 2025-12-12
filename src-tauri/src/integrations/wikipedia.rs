use reqwest;
use serde::{Deserialize, Serialize};
use log;

#[derive(Serialize, Deserialize, Debug, Clone)]
struct WikipediaQueryPage {
    pageid: Option<i64>,
    title: Option<String>,
    extract: Option<String>,
    missing: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct WikipediaQuery {
    pages: Vec<WikipediaQueryPage>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct WikipediaResponse {
    batchcomplete: Option<bool>,
    query: Option<WikipediaQuery>,
}

pub async fn perform_wikipedia_lookup(
    client: &reqwest::Client,
    search_term: &str,
) -> Result<Option<(String, String, String)>, String> {
    // (title, summary, source_url)
    let base_url = "https://en.wikipedia.org/w/api.php";
    let params = [
        ("action", "query"),
        ("format", "json"),
        ("titles", search_term),
        ("prop", "extracts"),
        ("exintro", "true"),
        ("explaintext", "true"),
        ("redirects", "1"),
        ("formatversion", "2"),
    ];

    log::info!("Performing Wikipedia lookup for: {}", search_term);

    match client
        .get(base_url)
        .query(&params)
        .header("User-Agent", "Shard/1.0 (https://github.com/shard-app/shard)")
        .send()
        .await
    {
        Ok(response) => {
            let status = response.status();
            let response_text = response
                .text()
                .await
                .map_err(|e| format!("Wikipedia: Failed to read response text: {}", e))?;

            if status.is_success() {
                match serde_json::from_str::<WikipediaResponse>(&response_text) {
                    Ok(wiki_response) => {
                        if let Some(query_data) = wiki_response.query {
                            if let Some(page) = query_data.pages.first() {
                                if page.missing.is_some() {
                                    log::info!("Wikipedia: Page '{}' does not exist.", search_term);
                                    return Ok(None);
                                }
                                if let Some(extract) = &page.extract {
                                    if !extract.trim().is_empty() {
                                        let title = page
                                            .title
                                            .clone()
                                            .unwrap_or_else(|| search_term.to_string());
                                        let source_url = format!(
                                            "https://en.wikipedia.org/wiki/{}",
                                            title.replace(" ", "_")
                                        );
                                        return Ok(Some((
                                            title,
                                            extract.trim().to_string(),
                                            source_url,
                                        )));
                                    }
                                }
                            }
                        }
                        Ok(None)
                    }
                    Err(e) => {
                        log::error!("Wikipedia: Failed to parse JSON: {}", e);
                        Err(format!("Wikipedia JSON parse error: {}", e))
                    }
                }
            } else {
                Err(format!("Wikipedia API error: {} - {}", status, response_text))
            }
        }
        Err(e) => Err(format!("Wikipedia network error: {}", e)),
    }
}
