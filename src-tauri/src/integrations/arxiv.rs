use reqwest;
use serde::{Deserialize, Serialize};
use log;
use regex::Regex;

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

    let (title, abstract_text, content) = parse_arxiv_html(&html, &id);

    Ok(ArxivPaperContent {
        id,
        title,
        abstract_text,
        content,
    })
}

/// Helper to extract clean text, filtering out MathML annotations
fn clean_text(element: scraper::ElementRef) -> String {
    // Tags that indicate MathML content (we skip all descendants of these)
    const SKIP_TAGS: &[&str] = &[
        "math", "annotation", "annotation-xml", "semantics",
        "mrow", "mi", "mo", "mn", "msub", "msup", "mfrac", "mstyle",
        "mspace", "mtext", "mover", "munder", "munderover", "mtable",
    ];

    let mut texts: Vec<String> = Vec::new();

    // Walk all descendants and collect text nodes
    for descendant in element.descendants() {
        if let Some(text) = descendant.value().as_text() {
            // Check if any ancestor is a MathML element by examining tag names
            let mut should_skip = false;
            let mut current = descendant.parent();
            while let Some(parent) = current {
                if let Some(el) = parent.value().as_element() {
                    let tag = el.name().to_lowercase();
                    if SKIP_TAGS.contains(&tag.as_str())
                       || el.has_class("ltx_Math", scraper::CaseSensitivity::AsciiCaseInsensitive)
                    {
                        should_skip = true;
                        break;
                    }
                }
                current = parent.parent();
            }

            if !should_skip {
                let t = text.trim();
                if !t.is_empty() {
                    texts.push(t.to_string());
                }
            }
        }
    }

    // Join and normalize whitespace
    let result = texts.join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");

    // Post-process: strip any remaining MathML-like noise patterns
    // This catches edge cases where tag names slipped through
    let mut result_cow = std::borrow::Cow::Borrowed(&result);

    if let Ok(re) = Regex::new(r"Node\s*\{[^}]*\}") {
        if re.is_match(&result_cow) {
            result_cow = std::borrow::Cow::Owned(re.replace_all(&result_cow, "").to_string());
        }
    }
    if let Ok(re) = Regex::new(r"Element\(<[^>]+>\)") {
        if re.is_match(&result_cow) {
            result_cow = std::borrow::Cow::Owned(re.replace_all(&result_cow, "").to_string());
        }
    }
    if let Ok(re) = Regex::new(r"NodeId\(\d+\)") {
        if re.is_match(&result_cow) {
            result_cow = std::borrow::Cow::Owned(re.replace_all(&result_cow, "").to_string());
        }
    }
    if let Ok(re) = Regex::new(r"Some\([^)]+\)") {
        if re.is_match(&result_cow) {
            result_cow = std::borrow::Cow::Owned(re.replace_all(&result_cow, "").to_string());
        }
    }

    let result = result_cow.to_string();

    // Clean up multiple spaces that may result from replacements
    result.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Parse ArXiv HTML content using an allowlist strategy
fn parse_arxiv_html(html: &str, id: &str) -> (String, String, String) {
    let document = scraper::Html::parse_document(html);

    // Extract title
    let title_selector = scraper::Selector::parse("h1.ltx_title_document").unwrap();
    let title = document
        .select(&title_selector)
        .next()
        .map(|el| clean_text(el))
        .unwrap_or_else(|| format!("Paper {}", id));

    // Extract abstract
    let abstract_selector = scraper::Selector::parse("div.ltx_abstract p.ltx_p").unwrap();
    let abstract_text = document
        .select(&abstract_selector)
        .next()
        .map(|el| clean_text(el))
        .unwrap_or_default();

    // Extract content using allowlist
    // We want: document title (already got it, but maybe useful for context),
    // section titles, subsection titles, and paragraphs.
    // We iterate in document order.
    let content_selector = scraper::Selector::parse(
        ".ltx_title_section, .ltx_title_subsection, .ltx_p"
    ).unwrap();

    let mut content_parts: Vec<String> = Vec::new();
    let mut char_count = 0;
    let max_chars = 30000; // Increased limit as we have cleaner text now

    for element in document.select(&content_selector) {
        if char_count >= max_chars {
            break;
        }

        let text = clean_text(element);
        if text.is_empty() {
            continue;
        }

        // Check element type to format accordingly
        let classes = element.value().classes().collect::<Vec<_>>();

        // Skip abstract paragraphs in the main content loop (we already have it)
        // Check if parent is ltx_abstract
        let mut is_abstract = false;
        if let Some(parent) = element.parent() {
            if let Some(parent_el) = parent.value().as_element() {
                if parent_el.has_class("ltx_abstract", scraper::CaseSensitivity::AsciiCaseInsensitive) {
                    is_abstract = true;
                }
            }
        }
        if is_abstract {
            continue;
        }

        let formatted = if classes.contains(&"ltx_title_section") {
            // Skip Reference/Bibliography sections
            if text.to_lowercase().contains("reference") || text.to_lowercase().contains("bibliograph") {
                // We might want to stop here or just skip this section header
                // For now, let's just skip the header, but subsequent paragraphs might still be included
                // if we don't track state.
                // Ideally we should track "current section" but for simplicity let's just skip the header.
                continue;
            }
            format!("\n## {}\n", text)
        } else if classes.contains(&"ltx_title_subsection") {
            format!("\n### {}\n", text)
        } else {
            // Paragraph
            // Skip very short paragraphs that might be noise/captions/etc if they aren't titles
            if text.len() < 20 {
                continue;
            }
            format!("{}\n", text)
        };

        char_count += formatted.len();
        content_parts.push(formatted);
    }

    let mut content = content_parts.join("\n");

    // Truncate at sentence boundary if needed
    if content.len() > max_chars {
        content = content.chars().take(max_chars).collect();
        if let Some(pos) = content.rfind(". ") {
            content.truncate(pos + 1);
        }
        content.push_str("\n\n[Content truncated...]");
    }

    // Fallback if nothing extracted
    if content.trim().is_empty() {
        content = format!(
            "[Paper content could not be extracted. View directly at: https://ar5iv.labs.arxiv.org/html/{}]",
            id
        );
    }

    (title, abstract_text, content)
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

    #[test]
    fn test_parse_arxiv_html_content() {
        let html = r#"
        <!DOCTYPE html>
        <html>
        <body>
            <h1 class="ltx_title_document">Test Paper Title</h1>
            <div class="ltx_abstract">
                <p class="ltx_p">This is the abstract.</p>
            </div>
            <section class="ltx_section">
                <h2 class="ltx_title_section">1. Introduction</h2>
                <div class="ltx_para">
                    <p class="ltx_p">
                        This is a paragraph with math:
                        <math><mi>x</mi><mo>+</mo><mn>1</mn></math>.
                        The math should be gone.
                    </p>
                </div>
            </section>
            <section class="ltx_section">
                <h2 class="ltx_title_section">2. Methods</h2>
                <div class="ltx_para">
                    <p class="ltx_p">Another paragraph that is definitely longer than twenty characters.</p>
                </div>
            </section>
        </body>
        </html>
        "#;

        let (title, abstract_text, content) = parse_arxiv_html(html, "test_id");

        assert_eq!(title, "Test Paper Title");
        assert_eq!(abstract_text, "This is the abstract.");

        // Check content structure
        assert!(content.contains("## 1. Introduction"));
        assert!(content.contains("This is a paragraph with math: . The math should be gone."));
        assert!(!content.contains("<math>"));
        assert!(!content.contains("x+1")); // Math content should be stripped
        assert!(content.contains("## 2. Methods"));
        assert!(content.contains("Another paragraph that is definitely longer than twenty characters."));
    }

    #[test]
    fn test_clean_text_removes_node_debug_strings() {
        // Simulate HTML where the text content looks like a Node debug dump
        // This shouldn't happen in reality, but if it does, we want to be sure we strip it.
        let html = r#"
        <div class="ltx_p">
            Some real text.
            Node { parent: Some(NodeId(7199)), value: Element(&lt;mi&gt;) }
            More real text.
        </div>
        "#;

        let document = scraper::Html::parse_document(html);
        let selector = scraper::Selector::parse("div").unwrap();
        let element = document.select(&selector).next().unwrap();

        let cleaned = clean_text(element);

        assert!(cleaned.contains("Some real text."));
        assert!(cleaned.contains("More real text."));
        assert!(!cleaned.contains("Node {"));
        assert!(!cleaned.contains("NodeId("));
        assert!(!cleaned.contains("Element(<mi>)"));
    }
}
