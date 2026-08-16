//! Host-free execution helpers for tools backed by external APIs.
//!
//! The Shard host owns lifecycle hooks, caching, frontend events, personas,
//! memory, persistence, availability, and heartbeat policy. This crate accepts
//! explicit clients, invocations, and credentials and returns tool output.

mod arxiv;
mod browser;
mod finance;
mod weather;
mod web_search;
mod wikipedia;
mod youtube;

use arxiv::{perform_arxiv_lookup, read_arxiv_paper};
use finance::perform_finance_lookup;
use shard_tool_api::ToolInvocation;
use weather::perform_weather_lookup;
use web_search::perform_web_search;
use wikipedia::perform_wikipedia_lookup;
pub use youtube::{fetch_youtube_transcript, YoutubeProcessConfig, YoutubeTranscriptToolOutput};

#[derive(Debug, Clone, Copy)]
pub struct ExternalToolConfig<'a> {
    pub brave_api_key: Option<&'a str>,
}

/// Execute an HTTP-backed external tool.
///
/// `None` means the host must continue its own dispatch. YouTube uses the
/// structured [`fetch_youtube_transcript`] API instead because the host
/// composes optional LLM summarization before rendering the final result.
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
                match perform_web_search(http_client, query, config.brave_api_key).await {
                    Ok(results) => serde_json::to_string(&results).unwrap_or_else(|_| {
                        "Failed to serialize search results to JSON".to_string()
                    }),
                    Err(e) => format!("Error: {}", e),
                },
            )
        }
        "open_url" => {
            let url = invocation.args["url"].as_str().unwrap_or_default();
            Some(match browser::read_url(http_client, url).await {
                Ok(markdown) => format!("Read URL Results for {}:\n\n{}", url, markdown),
                Err(e) => format!("Error reading URL: {}", e),
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    async fn dispatch(name: &str) -> Option<String> {
        let args = json!({});
        execute_external_tool(
            &reqwest::Client::new(),
            ToolInvocation { name, args: &args },
            ExternalToolConfig {
                brave_api_key: None,
            },
        )
        .await
    }

    #[tokio::test]
    async fn unknown_tools_fall_through_to_host_dispatch() {
        assert!(dispatch("save_memory").await.is_none());
    }

    #[tokio::test]
    async fn youtube_requires_host_summary_composition() {
        assert!(dispatch("youtube_transcript").await.is_none());
    }
}
