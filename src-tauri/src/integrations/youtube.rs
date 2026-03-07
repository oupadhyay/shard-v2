/**
 * YouTube Transcript Integration
 *
 * Fetches video transcripts using `yt-dlp` for metadata + direct caption URL fetch.
 * Flow:
 *   1. Extract video ID from URL or raw ID
 *   2. Run `yt-dlp -j` to get video metadata JSON (includes subtitle URLs)
 *   3. Pick the best caption track (manual English > auto English > first available)
 *   4. Fetch the XML transcript from the caption track URL (srv1 format)
 *   5. Parse XML into timestamped text segments
 */
use log;
use reqwest::Client;
use serde::Deserialize;

// ── Video ID extraction ─────────────────────────────────────────────

/// Extract a YouTube video ID from a URL or return the input if it already looks like an ID.
///
/// Handles: youtube.com/watch?v=, youtu.be/, youtube.com/embed/, youtube.com/shorts/,
/// and bare 11-char IDs.
pub fn extract_video_id(input: &str) -> Option<String> {
    let input = input.trim();

    // Try URL parsing first
    if let Ok(url) = reqwest::Url::parse(input) {
        let host = url.host_str().unwrap_or_default();

        // youtu.be/VIDEO_ID
        if host == "youtu.be" {
            let path = url.path().trim_start_matches('/');
            if !path.is_empty() {
                return Some(path.split('/').next()?.split('?').next()?.to_string());
            }
        }

        // youtube.com or www.youtube.com or m.youtube.com
        if host.contains("youtube.com") {
            // /watch?v=VIDEO_ID
            if url.path() == "/watch" {
                for (key, value) in url.query_pairs() {
                    if key == "v" && !value.is_empty() {
                        return Some(value.to_string());
                    }
                }
            }

            // /embed/VIDEO_ID or /shorts/VIDEO_ID or /v/VIDEO_ID
            let segments: Vec<&str> = url.path().trim_start_matches('/').split('/').collect();
            if segments.len() >= 2
                && matches!(segments[0], "embed" | "shorts" | "v")
                && !segments[1].is_empty()
            {
                return Some(segments[1].split('?').next()?.to_string());
            }
        }
    }

    // Bare video ID: 11 alphanumeric chars (plus - and _)
    if input.len() == 11 && input.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Some(input.to_string());
    }

    None
}

// ── yt-dlp metadata types ───────────────────────────────────────────

/// Subset of yt-dlp JSON output we care about.
#[derive(Debug, Deserialize)]
struct YtDlpMetadata {
    /// Manually uploaded subtitles, keyed by language code.
    #[serde(default)]
    subtitles: std::collections::HashMap<String, Vec<SubtitleEntry>>,
    /// Auto-generated captions, keyed by language code.
    #[serde(default)]
    automatic_captions: std::collections::HashMap<String, Vec<SubtitleEntry>>,
}

#[derive(Debug, Deserialize)]
struct SubtitleEntry {
    /// Download URL for this subtitle track.
    url: String,
    /// Format extension: "srv1", "json3", "vtt", etc.
    ext: String,
}

// ── Transcript XML types ────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct Transcript {
    #[serde(rename = "text", default)]
    segments: Vec<TranscriptSegment>,
}

#[derive(Debug, Deserialize)]
struct TranscriptSegment {
    #[serde(rename = "@start")]
    start: Option<String>,
    #[serde(rename = "@dur")]
    dur: Option<String>,
    #[serde(rename = "$text", default)]
    text: String,
}

// ── Public types ────────────────────────────────────────────────────

/// A single transcript segment with timing info.
#[derive(Debug, Clone)]
pub struct TimedSegment {
    pub start_secs: f64,
    pub duration_secs: f64,
    pub text: String,
}

// ── Core logic ──────────────────────────────────────────────────────

/// Fetch the transcript for a YouTube video.
///
/// Uses `yt-dlp -j` to get subtitle URLs, then fetches the XML caption track.
/// Prefers manual English captions, falls back to auto-generated, then first available.
pub async fn fetch_transcript(
    client: &Client,
    video_id: &str,
) -> Result<Vec<TimedSegment>, String> {
    log::info!("[YouTube] Fetching transcript for video: {}", video_id);

    // 1. Run yt-dlp to get video metadata JSON
    let video_url = format!("https://www.youtube.com/watch?v={}", video_id);
    let metadata = run_ytdlp(&video_url).await?;

    // 2. Pick the best caption track URL (srv1 XML format)
    let caption_url = pick_caption_url(&metadata)?;

    log::info!("[YouTube] Fetching caption XML from: {}", caption_url);

    // 3. Fetch the XML transcript
    let xml_resp = client
        .get(&caption_url)
        .send()
        .await
        .map_err(|e| format!("Failed to fetch transcript XML: {}", e))?;

    if !xml_resp.status().is_success() {
        return Err(format!(
            "Transcript XML returned HTTP {}",
            xml_resp.status()
        ));
    }

    let xml_text = xml_resp
        .text()
        .await
        .map_err(|e| format!("Failed to read transcript XML body: {}", e))?;

    // 4. Parse XML into segments
    let transcript: Transcript = quick_xml::de::from_str(&xml_text)
        .map_err(|e| format!("Failed to parse transcript XML: {}", e))?;

    let segments: Vec<TimedSegment> = transcript
        .segments
        .into_iter()
        .map(|seg| {
            let text = html_decode(&seg.text);
            TimedSegment {
                start_secs: seg.start.as_deref().unwrap_or("0").parse().unwrap_or(0.0),
                duration_secs: seg.dur.as_deref().unwrap_or("0").parse().unwrap_or(0.0),
                text,
            }
        })
        .collect();

    if segments.is_empty() {
        return Err("Transcript is empty.".to_string());
    }

    log::info!(
        "[YouTube] Parsed {} transcript segments",
        segments.len()
    );

    Ok(segments)
}

/// Run `yt-dlp -j <url>` and parse the JSON output.
async fn run_ytdlp(video_url: &str) -> Result<YtDlpMetadata, String> {
    let output = tokio::process::Command::new("yt-dlp")
        .args(["-j", "--no-warnings", "--skip-download", video_url])
        .output()
        .await
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                "yt-dlp is not installed. Install it with: brew install yt-dlp (macOS) or pip install yt-dlp".to_string()
            } else {
                format!("Failed to run yt-dlp: {}", e)
            }
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "yt-dlp failed (exit {}): {}",
            output.status.code().unwrap_or(-1),
            stderr.trim()
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&stdout)
        .map_err(|e| format!("Failed to parse yt-dlp JSON output: {}", e))
}

/// Pick the best caption URL from yt-dlp metadata.
///
/// Priority: manual English (srv1) > auto English (srv1) > any manual (srv1) > any auto (srv1).
fn pick_caption_url(metadata: &YtDlpMetadata) -> Result<String, String> {
    // Helper: find srv1 URL in a list of subtitle entries
    fn find_srv1(entries: &[SubtitleEntry]) -> Option<&str> {
        entries
            .iter()
            .find(|e| e.ext == "srv1")
            .map(|e| e.url.as_str())
    }

    // Helper: find srv1 URL for English in a subtitle map
    fn find_english_srv1(
        map: &std::collections::HashMap<String, Vec<SubtitleEntry>>,
    ) -> Option<String> {
        // Exact "en" first
        if let Some(entries) = map.get("en") {
            if let Some(url) = find_srv1(entries) {
                return Some(url.to_string());
            }
        }
        // Then en- variants (en-US, en-GB, etc.)
        for (lang, entries) in map {
            if lang.starts_with("en") {
                if let Some(url) = find_srv1(entries) {
                    return Some(url.to_string());
                }
            }
        }
        None
    }

    // Helper: find srv1 URL for any language in a subtitle map
    fn find_any_srv1(
        map: &std::collections::HashMap<String, Vec<SubtitleEntry>>,
    ) -> Option<String> {
        for entries in map.values() {
            if let Some(url) = find_srv1(entries) {
                return Some(url.to_string());
            }
        }
        None
    }

    // 1. Manual English
    if let Some(url) = find_english_srv1(&metadata.subtitles) {
        log::info!("[YouTube] Using manual English captions");
        return Ok(url);
    }

    // 2. Auto-generated English
    if let Some(url) = find_english_srv1(&metadata.automatic_captions) {
        log::info!("[YouTube] Using auto-generated English captions");
        return Ok(url);
    }

    // 3. Any manual language
    if let Some(url) = find_any_srv1(&metadata.subtitles) {
        log::info!("[YouTube] Using manual captions (non-English)");
        return Ok(url);
    }

    // 4. Any auto-generated language
    if let Some(url) = find_any_srv1(&metadata.automatic_captions) {
        log::info!("[YouTube] Using auto-generated captions (non-English)");
        return Ok(url);
    }

    Err("No captions available for this video. The video may not have subtitles or closed captions.".to_string())
}

/// Format transcript segments into a readable string with timestamps.
pub fn format_transcript(segments: &[TimedSegment]) -> String {
    let mut result = String::new();

    // Compute total video duration from last segment
    if let Some(last) = segments.last() {
        let total_secs = (last.start_secs + last.duration_secs) as u32;
        let total_mins = total_secs / 60;
        let total_rem = total_secs % 60;
        result.push_str(&format!(
            "Duration: {:02}:{:02}\n\n",
            total_mins, total_rem
        ));
    }

    for seg in segments {
        let mins = (seg.start_secs / 60.0) as u32;
        let secs = (seg.start_secs % 60.0) as u32;
        result.push_str(&format!("[{:02}:{:02}] {}\n", mins, secs, seg.text));
    }
    result
}

/// Decode common HTML entities that appear in YouTube transcript XML.
fn html_decode(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("\n", " ")
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_video_id_watch_url() {
        assert_eq!(
            extract_video_id("https://www.youtube.com/watch?v=dQw4w9WgXcQ"),
            Some("dQw4w9WgXcQ".to_string())
        );
    }

    #[test]
    fn test_extract_video_id_short_url() {
        assert_eq!(
            extract_video_id("https://youtu.be/dQw4w9WgXcQ"),
            Some("dQw4w9WgXcQ".to_string())
        );
    }

    #[test]
    fn test_extract_video_id_embed_url() {
        assert_eq!(
            extract_video_id("https://www.youtube.com/embed/dQw4w9WgXcQ"),
            Some("dQw4w9WgXcQ".to_string())
        );
    }

    #[test]
    fn test_extract_video_id_shorts_url() {
        assert_eq!(
            extract_video_id("https://www.youtube.com/shorts/dQw4w9WgXcQ"),
            Some("dQw4w9WgXcQ".to_string())
        );
    }

    #[test]
    fn test_extract_video_id_bare_id() {
        assert_eq!(
            extract_video_id("dQw4w9WgXcQ"),
            Some("dQw4w9WgXcQ".to_string())
        );
    }

    #[test]
    fn test_extract_video_id_with_extra_params() {
        assert_eq!(
            extract_video_id("https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=42"),
            Some("dQw4w9WgXcQ".to_string())
        );
    }

    #[test]
    fn test_extract_video_id_mobile_url() {
        assert_eq!(
            extract_video_id("https://m.youtube.com/watch?v=dQw4w9WgXcQ"),
            Some("dQw4w9WgXcQ".to_string())
        );
    }

    #[test]
    fn test_extract_video_id_invalid() {
        assert_eq!(extract_video_id("not a url"), None);
        assert_eq!(extract_video_id("https://google.com"), None);
        assert_eq!(extract_video_id(""), None);
    }

    #[test]
    fn test_pick_caption_url_prefers_manual_english() {
        let metadata = YtDlpMetadata {
            subtitles: {
                let mut m = std::collections::HashMap::new();
                m.insert(
                    "en".to_string(),
                    vec![
                        SubtitleEntry {
                            url: "https://example.com/manual.json3".to_string(),
                            ext: "json3".to_string(),
                        },
                        SubtitleEntry {
                            url: "https://example.com/manual.srv1".to_string(),
                            ext: "srv1".to_string(),
                        },
                    ],
                );
                m
            },
            automatic_captions: {
                let mut m = std::collections::HashMap::new();
                m.insert(
                    "en".to_string(),
                    vec![SubtitleEntry {
                        url: "https://example.com/auto.srv1".to_string(),
                        ext: "srv1".to_string(),
                    }],
                );
                m
            },
        };
        let url = pick_caption_url(&metadata).unwrap();
        assert_eq!(url, "https://example.com/manual.srv1");
    }

    #[test]
    fn test_pick_caption_url_falls_back_to_auto() {
        let metadata = YtDlpMetadata {
            subtitles: std::collections::HashMap::new(),
            automatic_captions: {
                let mut m = std::collections::HashMap::new();
                m.insert(
                    "en".to_string(),
                    vec![SubtitleEntry {
                        url: "https://example.com/auto.srv1".to_string(),
                        ext: "srv1".to_string(),
                    }],
                );
                m
            },
        };
        let url = pick_caption_url(&metadata).unwrap();
        assert_eq!(url, "https://example.com/auto.srv1");
    }

    #[test]
    fn test_pick_caption_url_no_captions() {
        let metadata = YtDlpMetadata {
            subtitles: std::collections::HashMap::new(),
            automatic_captions: std::collections::HashMap::new(),
        };
        let result = pick_caption_url(&metadata);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("No captions available"));
    }

    #[test]
    fn test_pick_caption_url_non_english_fallback() {
        let metadata = YtDlpMetadata {
            subtitles: {
                let mut m = std::collections::HashMap::new();
                m.insert(
                    "es".to_string(),
                    vec![SubtitleEntry {
                        url: "https://example.com/spanish.srv1".to_string(),
                        ext: "srv1".to_string(),
                    }],
                );
                m
            },
            automatic_captions: std::collections::HashMap::new(),
        };
        let url = pick_caption_url(&metadata).unwrap();
        assert_eq!(url, "https://example.com/spanish.srv1");
    }

    #[test]
    fn test_parse_ytdlp_json() {
        // Minimal yt-dlp JSON with subtitle entries
        let json = r#"{
            "subtitles": {
                "en": [
                    {"url": "https://example.com/en.srv1", "ext": "srv1"},
                    {"url": "https://example.com/en.json3", "ext": "json3"}
                ]
            },
            "automatic_captions": {
                "en": [
                    {"url": "https://example.com/auto-en.srv1", "ext": "srv1"}
                ]
            }
        }"#;
        let metadata: YtDlpMetadata = serde_json::from_str(json).unwrap();
        assert_eq!(metadata.subtitles.len(), 1);
        assert_eq!(metadata.subtitles["en"].len(), 2);
        assert_eq!(metadata.automatic_captions["en"][0].ext, "srv1");
    }

    #[test]
    fn test_parse_ytdlp_json_empty_captions() {
        // yt-dlp JSON with no subtitles at all
        let json = r#"{"subtitles": {}, "automatic_captions": {}}"#;
        let metadata: YtDlpMetadata = serde_json::from_str(json).unwrap();
        assert!(metadata.subtitles.is_empty());
        assert!(metadata.automatic_captions.is_empty());
    }

    #[test]
    fn test_html_decode() {
        assert_eq!(html_decode("hello &amp; world"), "hello & world");
        assert_eq!(html_decode("a &lt; b &gt; c"), "a < b > c");
        assert_eq!(html_decode("it&#39;s &quot;fine&quot;"), "it's \"fine\"");
    }

    #[test]
    fn test_format_transcript() {
        let segments = vec![
            TimedSegment {
                start_secs: 0.0,
                duration_secs: 3.5,
                text: "Hello world".to_string(),
            },
            TimedSegment {
                start_secs: 65.2,
                duration_secs: 2.0,
                text: "Second segment".to_string(),
            },
        ];
        let formatted = format_transcript(&segments);
        assert!(formatted.contains("Duration: 01:07"));
        assert!(formatted.contains("[00:00] Hello world"));
        assert!(formatted.contains("[01:05] Second segment"));
    }
}
