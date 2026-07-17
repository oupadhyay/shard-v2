//! Host-free execution helpers for tools backed by external APIs.
//!
//! The Tauri agent host owns lifecycle hooks, caching, frontend events,
//! personas, memory, and persistence. This module owns only network/API-backed
//! tool behavior so it can move to a separate external-tools crate later.

use crate::integrations::{
    arxiv::{perform_arxiv_lookup, read_arxiv_paper},
    finance::perform_finance_lookup,
    weather::perform_weather_lookup,
    web_search::perform_web_search,
    wikipedia::perform_wikipedia_lookup,
};
use crate::tool_api::ToolInvocation;

#[derive(Debug, Clone, Copy)]
pub struct ExternalToolConfig<'a> {
    pub brave_api_key: Option<&'a str>,
}

#[derive(Debug, Clone)]
pub struct YoutubeTranscriptToolOutput {
    pub video_id: String,
    pub video_link: String,
    pub title: Option<String>,
    pub channel: Option<String>,
    pub segment_count: usize,
    pub formatted: String,
}

impl YoutubeTranscriptToolOutput {
    pub fn title_label(&self) -> &str {
        self.title.as_deref().unwrap_or(&self.video_id)
    }

    fn source_label(&self) -> String {
        match self.channel.as_deref() {
            Some(channel) => format!("{} — {}", self.title_label(), channel),
            None => self.title_label().to_string(),
        }
    }

    pub fn char_count(&self) -> usize {
        self.formatted.chars().count()
    }

    pub fn render(&self, summary: Option<&str>) -> String {
        let char_count = self.char_count();
        if char_count > 30_000 {
            let truncate_at = self
                .formatted
                .char_indices()
                .nth(30_000)
                .map(|(i, _)| i)
                .unwrap_or(self.formatted.len());
            let summary_section = summary
                .map(|summary| {
                    format!(
                        "\n\n--- LLM Summary of Full Video ---\n\n{}\n\n--- End Summary ---",
                        summary
                    )
                })
                .unwrap_or_default();

            format!(
                "YouTube Transcript — {} ({})\n{} segments, truncated\n\n{}...\n\n[Transcript truncated at ~30,000 chars. Total length: {} chars]{}",
                self.source_label(),
                self.video_link,
                self.segment_count,
                &self.formatted[..truncate_at],
                char_count,
                summary_section,
            )
        } else {
            format!(
                "YouTube Transcript — {} ({})\n{} segments\n\n{}",
                self.source_label(),
                self.video_link,
                self.segment_count,
                self.formatted
            )
        }
    }
}

pub async fn execute_external_tool(
    http_client: &reqwest::Client,
    invocation: ToolInvocation<'_>,
    config: ExternalToolConfig<'_>,
) -> Option<String> {
    match invocation.name {
        "get_weather" => {
            let location = invocation.args["location"].as_str().unwrap_or_default();
            Some(match perform_weather_lookup(http_client, location).await {
                Ok(json_str) => json_str,
                Err(e) => format!("Error: {}", e),
            })
        }
        "search_wikipedia" => {
            let query = invocation.args["query"].as_str().unwrap_or_default();
            Some(match perform_wikipedia_lookup(http_client, query).await {
                Ok(Some((title, summary, _))) => {
                    format!("Wikipedia Title: {}\nSummary: {}", title, summary)
                }
                Ok(None) => "No Wikipedia results found.".to_string(),
                Err(e) => format!("Error: {}", e),
            })
        }
        "get_stock_price" => {
            let symbol = invocation.args["symbol"].as_str().unwrap_or_default();
            Some(
                perform_finance_lookup(symbol)
                    .await
                    .unwrap_or_else(|e| format!("Error: {}", e)),
            )
        }
        "search_arxiv" => {
            let query = invocation.args["query"].as_str().unwrap_or_default();
            Some(match perform_arxiv_lookup(http_client, query, 3).await {
                Ok(papers) => {
                    let summaries: Vec<String> = papers
                        .iter()
                        .map(|p| {
                            format!(
                                "- [{}] {} ({}): {}",
                                p.id,
                                p.title,
                                p.published_date.as_deref().unwrap_or("?"),
                                p.summary
                            )
                        })
                        .collect();
                    format!("ArXiv Results:\n{}", summaries.join("\n\n"))
                }
                Err(e) => format!("Error: {}", e),
            })
        }
        "read_arxiv_paper" => {
            let paper_id = invocation.args["paper_id"].as_str().unwrap_or_default();
            Some(match read_arxiv_paper(http_client, paper_id).await {
                Ok(paper) => {
                    format!(
                        "# {}\n\n**Abstract:** {}\n\n{}",
                        paper.title, paper.abstract_text, paper.content
                    )
                }
                Err(e) => format!("Error reading paper: {}", e),
            })
        }
        "web_search" => {
            let query = invocation.args["query"].as_str().unwrap_or_default();
            Some(
                match perform_web_search(query, config.brave_api_key).await {
                    Ok(results) => serde_json::to_string(&results).unwrap_or_else(|_| {
                        "Failed to serialize search results to JSON".to_string()
                    }),
                    Err(e) => format!("Error: {}", e),
                },
            )
        }
        "open_url" => {
            let url = invocation.args["url"].as_str().unwrap_or_default();
            Some(
                match crate::integrations::browser::read_url(http_client, url).await {
                    Ok(markdown) => format!("Read URL Results for {}:\n\n{}", url, markdown),
                    Err(e) => format!("Error reading URL: {}", e),
                },
            )
        }
        _ => None,
    }
}

pub async fn fetch_youtube_transcript(
    http_client: &reqwest::Client,
    video: &str,
) -> Result<YoutubeTranscriptToolOutput, String> {
    let video_id = crate::integrations::youtube::extract_video_id(video).ok_or_else(|| {
        format!(
            "Error: Could not extract a YouTube video ID from '{}'",
            video
        )
    })?;

    let result = crate::integrations::youtube::fetch_transcript(http_client, &video_id).await?;
    let formatted = crate::integrations::youtube::format_transcript(
        &result.segments,
        result.title.as_deref(),
        result.channel.as_deref(),
    );

    Ok(YoutubeTranscriptToolOutput {
        video_link: format!("https://youtu.be/{}", video_id),
        video_id,
        title: result.title,
        channel: result.channel,
        segment_count: result.segments.len(),
        formatted,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn youtube_transcript_render_truncates_on_char_boundary_and_appends_summary() {
        let output = YoutubeTranscriptToolOutput {
            video_id: "abc123".to_string(),
            video_link: "https://youtu.be/abc123".to_string(),
            title: Some("Title".to_string()),
            channel: None,
            segment_count: 2,
            formatted: format!("{}é", "a".repeat(30_000)),
        };

        let rendered = output.render(Some("summary"));

        assert!(rendered.contains("YouTube Transcript — Title (https://youtu.be/abc123)"));
        assert!(
            rendered.contains("[Transcript truncated at ~30,000 chars. Total length: 30001 chars]")
        );
        assert!(rendered.contains("--- LLM Summary of Full Video ---\n\nsummary"));
    }
}
