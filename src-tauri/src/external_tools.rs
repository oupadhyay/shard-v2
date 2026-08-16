//! Compatibility surface for portable external tools and host-owned YouTube policy.
//!
//! The Tauri agent host owns lifecycle hooks, caching, frontend events,
//! personas, memory, persistence, and YouTube summarization/presentation. HTTP
//! API-backed tool execution is re-exported from `shard-external-tools`.

pub use crate::integrations::youtube::YoutubeProcessConfig;
pub use shard_external_tools::{execute_external_tool, ExternalToolConfig};

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

pub async fn fetch_youtube_transcript(
    http_client: &reqwest::Client,
    video: &str,
    process_config: &YoutubeProcessConfig,
) -> Result<YoutubeTranscriptToolOutput, String> {
    let video_id = crate::integrations::youtube::extract_video_id(video).ok_or_else(|| {
        format!(
            "Error: Could not extract a YouTube video ID from '{}'",
            video
        )
    })?;

    let result =
        crate::integrations::youtube::fetch_transcript(http_client, &video_id, process_config)
            .await?;
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
