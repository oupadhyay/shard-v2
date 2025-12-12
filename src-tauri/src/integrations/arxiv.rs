use reqwest;
use serde::{Deserialize, Serialize};
use log;

// ArXiv Atom XML Structs (Ported from legacy)
#[derive(Debug, Deserialize)]
pub enum FeedChild {
    #[serde(rename = "entry")]
    Entry(ArxivEntry),
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize, Default)]
pub struct ArxivFeed {
    #[serde(rename = "$value", default)]
    pub children: Vec<FeedChild>,
}

#[derive(Debug, Deserialize, Default)]
pub struct ArxivEntry {
    pub id: Option<String>,
    // updated: Option<String>,
    pub published: Option<String>,
    pub title: Option<String>,
    pub summary: Option<String>,
    #[serde(rename = "author", default)]
    pub authors: Vec<ArxivAuthor>,
    #[serde(rename = "link", default)]
    pub entry_links: Vec<ArxivLink>,
}

#[derive(Debug, Deserialize, Default)]
pub struct ArxivAuthor {
    pub name: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct ArxivLink {
    #[serde(rename = "@href")]
    pub href: Option<String>,
    #[serde(rename = "@title")]
    pub title: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ArxivPaperSummary {
    pub title: String,
    pub summary: String,
    pub authors: Vec<String>,
    pub id: String,
    pub published_date: Option<String>,
    pub pdf_url: String,
}

pub async fn perform_arxiv_lookup(
    client: &reqwest::Client,
    query: &str,
    max_results: usize,
) -> Result<Vec<ArxivPaperSummary>, String> {
    let base_url = "http://export.arxiv.org/api/query";
    let params = [
        ("search_query", query),
        ("start", "0"),
        ("max_results", &max_results.to_string()),
    ];

    log::info!("Performing ArXiv lookup for: {}", query);

    let response = client
        .get(base_url)
        .query(&params)
        .send()
        .await
        .map_err(|e| format!("ArXiv network error: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("ArXiv API error: {}", response.status()));
    }

    let response_text = response
        .text()
        .await
        .map_err(|e| format!("ArXiv read error: {}", e))?;

    // Parse XML
    let feed: ArxivFeed = quick_xml::de::from_str(&response_text)
        .map_err(|e| format!("ArXiv XML parse error: {}", e))?;

    let mut summaries = Vec::new();

    for child in feed.children {
        if let FeedChild::Entry(entry) = child {
            let title = entry.title.unwrap_or_default().replace("\n", " ").trim().to_string();
            let summary = entry.summary.unwrap_or_default().replace("\n", " ").trim().to_string();
            let authors = entry.authors.into_iter().filter_map(|a| a.name).collect();
            let id = entry.id.unwrap_or_default();
            let published_date = entry.published;

            let pdf_url = entry.entry_links
                .iter()
                .find(|l| l.title.as_deref() == Some("pdf"))
                .and_then(|l| l.href.clone())
                .unwrap_or_default();

            summaries.push(ArxivPaperSummary {
                title,
                summary,
                authors,
                id,
                published_date,
                pdf_url,
            });
        }
    }

    Ok(summaries)
}
