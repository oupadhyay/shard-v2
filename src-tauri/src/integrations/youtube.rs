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

/// Validate that a string looks like a YouTube video ID (11 alphanumeric chars including - and _).
fn is_valid_video_id(id: &str) -> bool {
    id.len() == 11 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

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
                let id = path.split('/').next()?.split('?').next()?.to_string();
                if is_valid_video_id(&id) {
                    return Some(id);
                }
            }
        }

        // youtube.com or www.youtube.com or m.youtube.com
        if host == "youtube.com" || host.ends_with(".youtube.com") {
            // /watch?v=VIDEO_ID
            if url.path() == "/watch" {
                for (key, value) in url.query_pairs() {
                    if key == "v" && !value.is_empty() {
                        let id = value.to_string();
                        if is_valid_video_id(&id) {
                            return Some(id);
                        }
                    }
                }
            }

            // /embed/VIDEO_ID or /shorts/VIDEO_ID or /v/VIDEO_ID
            let segments: Vec<&str> = url.path().trim_start_matches('/').split('/').collect();
            if segments.len() >= 2
                && matches!(segments[0], "embed" | "shorts" | "v")
                && !segments[1].is_empty()
            {
                let id = segments[1].split('?').next()?.to_string();
                if is_valid_video_id(&id) {
                    return Some(id);
                }
            }
        }
    }

    // Bare video ID: 11 alphanumeric chars (plus - and _)
    if is_valid_video_id(input) {
        return Some(input.to_string());
    }

    None
}

// ── yt-dlp metadata types ───────────────────────────────────────────

/// Subset of yt-dlp JSON output we care about.
#[derive(Debug, Deserialize)]
struct YtDlpMetadata {
    /// Video title.
    #[serde(default)]
    title: Option<String>,
    /// Channel/uploader name.
    #[serde(default)]
    channel: Option<String>,
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

/// Result of fetching a transcript, including video metadata.
#[derive(Debug)]
pub struct TranscriptResult {
    pub title: Option<String>,
    pub channel: Option<String>,
    pub segments: Vec<TimedSegment>,
}

// ── Core logic ──────────────────────────────────────────────────────

/// Fetch the transcript for a YouTube video.
///
/// Uses `yt-dlp -j` to get subtitle URLs, then fetches the XML caption track.
/// Prefers manual English captions, falls back to auto-generated, then first available.
pub async fn fetch_transcript(
    client: &Client,
    video_id: &str,
) -> Result<TranscriptResult, String> {
    log::info!("[YouTube] Fetching transcript for video: {}", video_id);

    // 1. Run yt-dlp to get video metadata JSON
    let video_url = format!("https://www.youtube.com/watch?v={}", video_id);
    let metadata = run_ytdlp(&video_url).await?;

    let title = metadata.title.clone();
    let channel = metadata.channel.clone();

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
        "[YouTube] Parsed {} transcript segments for \"{}\"",
        segments.len(),
        title.as_deref().unwrap_or("unknown")
    );

    Ok(TranscriptResult {
        title,
        channel,
        segments,
    })
}

/// Run `yt-dlp -j <url>` and parse the JSON output.
/// Times out after 30 seconds to prevent indefinite hangs.
async fn run_ytdlp(video_url: &str) -> Result<YtDlpMetadata, String> {
    let mut child = tokio::process::Command::new("yt-dlp")
        .args(["-j", "--no-warnings", "--skip-download", video_url])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                "yt-dlp is not installed. Install it with: brew install yt-dlp (macOS) or pip install yt-dlp".to_string()
            } else {
                format!("Failed to run yt-dlp: {}", e)
            }
        })?;

    // Read stdout/stderr concurrently with waiting for the child to avoid
    // deadlocks if OS pipe buffers fill up.
    use tokio::io::AsyncReadExt;

    let mut stdout = child.stdout.take().ok_or("Failed to capture yt-dlp stdout")?;
    let mut stderr = child.stderr.take().ok_or("Failed to capture yt-dlp stderr")?;

    let join_fut = async {
        let mut stdout_buf = Vec::new();
        let mut stderr_buf = Vec::new();

        let wait_fut = child.wait();
        let stdout_fut = stdout.read_to_end(&mut stdout_buf);
        let stderr_fut = stderr.read_to_end(&mut stderr_buf);

        let (status_res, stdout_res, stderr_res) =
            tokio::join!(wait_fut, stdout_fut, stderr_fut);

        let status = status_res?;
        stdout_res?;
        stderr_res?;

        Ok::<(std::process::ExitStatus, Vec<u8>, Vec<u8>), std::io::Error>((
            status,
            stdout_buf,
            stderr_buf,
        ))
    };

    let timeout = std::time::Duration::from_secs(30);
    let (status, stdout_buf, stderr_buf) =
        match tokio::time::timeout(timeout, join_fut).await {
            Err(_) => {
                let _ = child.kill().await;
                return Err("yt-dlp timed out after 30 seconds".to_string());
            }
            Ok(Err(e)) => {
                return Err(format!("Failed to run yt-dlp: {}", e));
            }
            Ok(Ok((status, stdout_buf, stderr_buf))) => (status, stdout_buf, stderr_buf),
        };

    if !status.success() {
        let stderr_str = String::from_utf8_lossy(&stderr_buf);
        return Err(format!(
            "yt-dlp failed (exit {}): {}",
            status.code().unwrap_or(-1),
            stderr_str.trim()
        ));
    }

    let stdout_str = String::from_utf8_lossy(&stdout_buf);
    serde_json::from_str(&stdout_str)
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
        let mut keys: Vec<&String> = map.keys().collect();
        keys.sort();
        for key in keys {
            if let Some(entries) = map.get(key) {
                if let Some(url) = find_srv1(entries) {
                    return Some(url.to_string());
                }
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

/// Format a duration in seconds to a human-readable timestamp.
/// When `force_hours` is true, always uses HH:MM:SS format.
/// Otherwise uses HH:MM:SS for durations >= 1 hour, MM:SS for shorter.
fn format_timestamp(total_secs: u32, force_hours: bool) -> String {
    let hours = total_secs / 3600;
    let mins = (total_secs % 3600) / 60;
    let secs = total_secs % 60;
    if force_hours || hours > 0 {
        format!("{:02}:{:02}:{:02}", hours, mins, secs)
    } else {
        format!("{:02}:{:02}", mins, secs)
    }
}

/// Format transcript segments into a readable string with timestamps.
/// Includes video metadata header if title/channel are provided.
pub fn format_transcript(
    segments: &[TimedSegment],
    title: Option<&str>,
    channel: Option<&str>,
) -> String {
    let mut result = String::new();

    // Header with title and channel
    if let Some(t) = title {
        result.push_str(&format!("Title: {}\n", t));
    }
    if let Some(c) = channel {
        result.push_str(&format!("Channel: {}\n", c));
    }

    // Compute total video duration from last segment
    let use_hours = segments
        .last()
        .map(|last| (last.start_secs + last.duration_secs) >= 3600.0)
        .unwrap_or(false);

    if let Some(last) = segments.last() {
        let total_secs = (last.start_secs + last.duration_secs) as u32;
        result.push_str(&format!("Duration: {}\n", format_timestamp(total_secs, false)));
    }

    result.push('\n');

    for seg in segments {
        let seg_secs = seg.start_secs as u32;
        let ts = format_timestamp(seg_secs, use_hours);
        result.push_str(&format!("[{}] {}\n", ts, seg.text));
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
        assert_eq!(extract_video_id("https://youtube.com.evil.tld/watch?v=dQw4w9WgXcQ"), None);
    }

    #[test]
    fn test_pick_caption_url_prefers_manual_english() {
        let metadata = YtDlpMetadata {
            title: None,
            channel: None,
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
            title: None,
            channel: None,
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
            title: None,
            channel: None,
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
            title: None,
            channel: None,
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
        // Minimal yt-dlp JSON with subtitle entries and title
        let json = r#"{
            "title": "Test Video",
            "channel": "Test Channel",
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
        assert_eq!(metadata.title.as_deref(), Some("Test Video"));
        assert_eq!(metadata.channel.as_deref(), Some("Test Channel"));
        assert_eq!(metadata.subtitles.len(), 1);
        assert_eq!(metadata.subtitles["en"].len(), 2);
        assert_eq!(metadata.automatic_captions["en"][0].ext, "srv1");
    }

    #[test]
    fn test_parse_ytdlp_json_empty_captions() {
        // yt-dlp JSON with no subtitles at all (title/channel optional)
        let json = r#"{"subtitles": {}, "automatic_captions": {}}"#;
        let metadata: YtDlpMetadata = serde_json::from_str(json).unwrap();
        assert!(metadata.subtitles.is_empty());
        assert!(metadata.automatic_captions.is_empty());
        assert!(metadata.title.is_none());
        assert!(metadata.channel.is_none());
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
        let formatted = format_transcript(&segments, None, None);
        assert!(formatted.contains("Duration: 01:07"));
        assert!(formatted.contains("[00:00] Hello world"));
        assert!(formatted.contains("[01:05] Second segment"));
        // No title/channel header when None
        assert!(!formatted.contains("Title:"));
        assert!(!formatted.contains("Channel:"));
    }

    #[test]
    fn test_format_transcript_with_metadata() {
        let segments = vec![
            TimedSegment {
                start_secs: 0.0,
                duration_secs: 5.0,
                text: "Welcome".to_string(),
            },
        ];
        let formatted = format_transcript(&segments, Some("My Video"), Some("My Channel"));
        assert!(formatted.contains("Title: My Video"));
        assert!(formatted.contains("Channel: My Channel"));
        assert!(formatted.contains("Duration: 00:05"));
        assert!(formatted.contains("[00:00] Welcome"));
    }

    #[test]
    fn test_format_transcript_hour_long_video() {
        let segments = vec![
            TimedSegment {
                start_secs: 0.0,
                duration_secs: 2.0,
                text: "Start".to_string(),
            },
            TimedSegment {
                start_secs: 3661.0,
                duration_secs: 5.0,
                text: "Over an hour in".to_string(),
            },
        ];
        let formatted = format_transcript(&segments, None, None);
        assert!(formatted.contains("Duration: 01:01:06"));
        assert!(formatted.contains("[00:00:00] Start"));
        assert!(formatted.contains("[01:01:01] Over an hour in"));
    }

    #[test]
    fn test_format_timestamp() {
        // Auto mode (force_hours = false): MM:SS under an hour, HH:MM:SS at/above
        assert_eq!(format_timestamp(0, false), "00:00");
        assert_eq!(format_timestamp(65, false), "01:05");
        assert_eq!(format_timestamp(3599, false), "59:59");
        assert_eq!(format_timestamp(3600, false), "01:00:00");
        assert_eq!(format_timestamp(3661, false), "01:01:01");
        assert_eq!(format_timestamp(7325, false), "02:02:05");
    }

    #[test]
    fn test_format_timestamp_forced_hours() {
        // force_hours = true: always HH:MM:SS even for short durations
        assert_eq!(format_timestamp(0, true), "00:00:00");
        assert_eq!(format_timestamp(65, true), "00:01:05");
        assert_eq!(format_timestamp(3599, true), "00:59:59");
        assert_eq!(format_timestamp(3600, true), "01:00:00");
    }
}
