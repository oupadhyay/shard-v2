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

/// Struct for full paper content
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ArxivPaperContent {
    pub id: String,
    pub title: String,
    pub abstract_text: String,
    pub content: String, // Truncated full text
}

/// Extract ArXiv ID from various input formats
/// Handles: arxiv.org/abs/2401.12345, ar5iv.org/abs/2401.12345, http://arxiv.org/abs/2401.12345v1, raw ID
pub fn extract_arxiv_id(input: &str) -> Option<String> {
    let input = input.trim();

    // Pattern: URLs like arxiv.org/abs/XXXX or ar5iv.org/abs/XXXX or html/XXXX
    if input.contains("arxiv.org") || input.contains("ar5iv") {
        // Extract the ID part after /abs/ or /html/
        if let Some(pos) = input.find("/abs/").or_else(|| input.find("/html/")) {
            let after = &input[pos + 5..]; // Skip "/abs/" or "/html"
            let after = after.trim_start_matches('/');
            // Take until next slash, query param, or end - also strip version suffix like v1
            let id: String = after
                .split(|c| c == '/' || c == '?' || c == '#')
                .next()
                .unwrap_or(after)
                .to_string();
            // Strip version suffix (e.g., "2401.12345v1" -> "2401.12345")
            let id = id.split('v').next().unwrap_or(&id).to_string();
            if !id.is_empty() {
                return Some(id);
            }
        }
    }

    // Pattern: raw ID like "2401.12345" or "hep-th/9901001"
    // New format: YYMM.NNNNN
    if input.chars().take(4).all(|c| c.is_ascii_digit())
        && input.chars().nth(4) == Some('.')
    {
        return Some(input.split('v').next().unwrap_or(input).to_string());
    }

    // Old format: category/XXXXXXX (e.g., hep-th/9901001)
    if input.contains('/') && !input.contains("://") {
        return Some(input.to_string());
    }

    None
}

/// Read full paper content from ar5iv (ArXiv HTML version)
pub async fn read_arxiv_paper(
    client: &reqwest::Client,
    paper_id_or_url: &str,
) -> Result<ArxivPaperContent, String> {
    let id = extract_arxiv_id(paper_id_or_url)
        .ok_or_else(|| format!("Could not extract ArXiv ID from: {}", paper_id_or_url))?;

    let url = format!("https://ar5iv.labs.arxiv.org/html/{}", id);
    log::info!("Fetching ArXiv paper from ar5iv: {}", url);

    let response = client
        .get(&url)
        .header("User-Agent", "Mozilla/5.0 (compatible; Shard/1.0)")
        .send()
        .await
        .map_err(|e| format!("ar5iv network error: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("ar5iv error: {} for paper {}", response.status(), id));
    }

    let html = response
        .text()
        .await
        .map_err(|e| format!("ar5iv read error: {}", e))?;

    // Parse HTML with scraper
    let document = scraper::Html::parse_document(&html);

    // Extract title - try multiple selectors
    let title = document
        .select(&scraper::Selector::parse("h1.ltx_title").unwrap())
        .next()
        .or_else(|| document.select(&scraper::Selector::parse("title").unwrap()).next())
        .map(|el| el.text().collect::<String>())
        .unwrap_or_else(|| format!("Paper {}", id))
        .trim()
        .to_string();

    // Extract abstract
    let abstract_text = document
        .select(&scraper::Selector::parse(".ltx_abstract").unwrap())
        .next()
        .map(|el| {
            el.text()
                .collect::<String>()
                .replace("Abstract", "")
                .trim()
                .to_string()
        })
        .unwrap_or_default();

    // Extract main content - sections and paragraphs
    let mut content_parts: Vec<String> = Vec::new();

    // Get section headings and their content
    let section_selector = scraper::Selector::parse("section.ltx_section, section.ltx_subsection").unwrap();
    let heading_selector = scraper::Selector::parse("h2, h3, h4").unwrap();
    let para_selector = scraper::Selector::parse("p.ltx_p").unwrap();

    for section in document.select(&section_selector) {
        // Get section heading
        if let Some(heading) = section.select(&heading_selector).next() {
            let heading_text = heading.text().collect::<String>().trim().to_string();
            if !heading_text.is_empty() {
                content_parts.push(format!("\n## {}\n", heading_text));
            }
        }

        // Get paragraphs in this section (limit to avoid too much content)
        for para in section.select(&para_selector).take(5) {
            let para_text = para.text().collect::<String>().trim().to_string();
            if !para_text.is_empty() && para_text.len() > 50 {
                content_parts.push(para_text);
            }
        }
    }

    // Join and truncate to 10k chars
    let mut content = content_parts.join("\n\n");
    if content.len() > 10000 {
        content.truncate(10000);
        content.push_str("\n\n[Content truncated...]");
    }

    // Fallback: if no structured content found, get all paragraph text
    if content.is_empty() {
        let all_paras: String = document
            .select(&scraper::Selector::parse("p").unwrap())
            .take(30)
            .map(|p| p.text().collect::<String>())
            .collect::<Vec<_>>()
            .join("\n\n");
        content = if all_paras.len() > 10000 {
            let mut s = all_paras[..10000].to_string();
            s.push_str("\n\n[Content truncated...]");
            s
        } else {
            all_paras
        };
    }

    Ok(ArxivPaperContent {
        id,
        title,
        abstract_text,
        content,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_arxiv_id_from_abs_url() {
        assert_eq!(
            extract_arxiv_id("https://arxiv.org/abs/2401.12345"),
            Some("2401.12345".to_string())
        );
        assert_eq!(
            extract_arxiv_id("http://arxiv.org/abs/2401.12345v2"),
            Some("2401.12345".to_string())
        );
    }

    #[test]
    fn test_extract_arxiv_id_from_ar5iv_url() {
        assert_eq!(
            extract_arxiv_id("https://ar5iv.labs.arxiv.org/html/2401.12345"),
            Some("2401.12345".to_string())
        );
        assert_eq!(
            extract_arxiv_id("https://ar5iv.org/abs/1910.06709"),
            Some("1910.06709".to_string())
        );
    }

    #[test]
    fn test_extract_arxiv_id_raw() {
        assert_eq!(
            extract_arxiv_id("2401.12345"),
            Some("2401.12345".to_string())
        );
        assert_eq!(
            extract_arxiv_id("hep-th/9901001"),
            Some("hep-th/9901001".to_string())
        );
    }

    #[test]
    fn test_extract_arxiv_id_invalid() {
        assert_eq!(extract_arxiv_id("not a valid id"), None);
        assert_eq!(extract_arxiv_id("https://google.com"), None);
    }
}
