/// Screen context capture and analysis module
/// Captures screen on window activation and uses Vision LLM to generate prompt suggestions
use base64::Engine;
use std::io::Cursor;
use std::sync::Mutex;
use std::time::Instant;

use crate::config::AppConfig;
use crate::integrations::vision_llm;
use ocr_rs::{OcrEngine, OcrEngineConfig};
use tauri::Manager;

/// Represents analyzed screen context with suggestions
#[derive(Debug, Clone, serde::Serialize)]
pub struct ScreenContext {
    pub capture_time_ms: u64,
    pub suggestions: Vec<String>,
    pub context_summary: String,
    pub image_base64: String,
    pub mime_type: String,
    pub ocr_text: String,
}

/// Cached context with debounce tracking
struct ContextCache {
    last_capture: Option<Instant>,
    last_context: Option<ScreenContext>,
}

lazy_static::lazy_static! {
    static ref CONTEXT_CACHE: Mutex<ContextCache> = Mutex::new(ContextCache {
        last_capture: None,
        last_context: None,
    });
}

/// Minimum time between captures (debounce)
const CAPTURE_DEBOUNCE_MS: u64 = 5000;

/// Cache TTL for context results
const CACHE_TTL_MS: u64 = 20000;

/// Environment variable to enable debug screenshot output
/// Set SHARD_DEBUG_SCREENSHOTS=1 to write screenshots to disk
const SHARD_DEBUG_SCREENSHOTS_ENV: &str = "SHARD_DEBUG_SCREENSHOTS";

/// Load the OCR engine using bundled PaddleOCR models
async fn get_ocr_engine(app_handle: &tauri::AppHandle) -> Result<OcrEngine, String> {
    let resource_dir = app_handle.path().resource_dir()
        .map_err(|e| format!("Failed to get resource directory: {}", e))?;

    let det_path = resource_dir.join("resources/ch_PP-OCRv4_det_infer.mnn");
    let rec_path = resource_dir.join("resources/ch_PP-OCRv4_rec_infer.mnn");
    let keys_path = resource_dir.join("resources/ppocr_keys_v4.txt");

    let det_path_str = det_path.to_str().ok_or("Invalid detection model path")?;
    let rec_path_str = rec_path.to_str().ok_or("Invalid recognition model path")?;
    let keys_path_str = keys_path.to_str().ok_or("Invalid keys file path")?;

    // Enable GPU (Metal) on macOS
    let config = OcrEngineConfig::gpu();

    OcrEngine::new(
        det_path_str,
        rec_path_str,
        keys_path_str,
        Some(config),
    )
    .map_err(|e| format!("Failed to initialize OCR engine: {}", e))
}

/// Vision LLM prompt for screen context analysis
const SCREEN_CONTEXT_PROMPT: &str = r#"You are an expert assistant generating prompt suggestions based on a user's screen context. You will be provided with a structured OCR extraction of the screen.

CRITICAL RULES:
1. PRIMARY FOCUS: Use ONLY the text under the "[Primary Window: ...]" section to generate suggestions.
2. IGNORE BACKGROUND: Completely ignore any text under the "[Background/Secondary]" section. These are inactive tabs or background windows and are NOT relevant.
3. NO HALLUCINATIONS: Do not suggest tasks based on your internal knowledge if they aren't directly supported by the Primary Window text.
4. BE DIRECT & CONCISE: Start suggestions with a strong verb (e.g., "Explain", "Rewrite", "Fix", "Summarize").
5. NO PREFIXES: DO NOT use prefixes like "Ask Claude to...", "Ask the assistant to...", or "Instruct the model to...". The user already knows they are talking to an AI.
6. NO GENERIC UI: Ignore menu bars, window controls, and system icons.

Return ONLY valid JSON:
{"summary": "concise description of the PRIMARY window activity", "suggestions": ["specific prompt 1", "specific prompt 2", "specific prompt 3"]}"#;

/// Capture the primary screen and return as base64 PNG and raw RgbImage
pub fn capture_screen() -> Result<(String, String, image::RgbImage), String> {
    let screens =
        screenshots::Screen::all().map_err(|e| format!("Failed to get screens: {}", e))?;

    let screen = screens.first().ok_or("No screens found")?;

    log::info!(
        "[ScreenContext] Capturing screen ID: {}, displays found: {}",
        screen.display_info.id,
        screens.len()
    );

    let image = screen
        .capture()
        .map_err(|e| format!("Failed to capture screen: {}", e))?;

    let width = image.width();
    let height = image.height();

    // OPTIMIZATION: Only 2 copies instead of 3
    // Copy 1: For resize/encode processing (will be consumed)
    // Copy 2: For OCR return value (full resolution)
    let rgba_data_for_ocr = image.rgba().to_vec();
    let rgba_data_for_resize = rgba_data_for_ocr.clone();

    log::info!("[ScreenContext] Captured {}x{} image", width, height);

    // Save FULL resolution debug image in BACKGROUND (only when env var is set)
    if std::env::var(SHARD_DEBUG_SCREENSHOTS_ENV).is_ok() {
        log::warn!("[ScreenContext] DEBUG: Writing full-resolution screenshot to disk");
        let rgba_data_for_debug = rgba_data_for_ocr.clone();
        std::thread::spawn(move || {
            // Use app cache dir instead of world-readable /tmp
            if let Some(cache_dir) = dirs::cache_dir() {
                let debug_dir = cache_dir.join("shard").join("debug");
                let _ = std::fs::create_dir_all(&debug_dir);
                let full_debug_path = debug_dir.join("shard_screen_debug_full.jpg");
                if let Some(full_rgba) = image::RgbaImage::from_raw(width, height, rgba_data_for_debug) {
                    let _ = image::DynamicImage::ImageRgba8(full_rgba)
                        .to_rgb8()
                        .save(full_debug_path);
                }
            }
        });
    }

    // Create image buffer for resize processing
    let rgba_image = image::RgbaImage::from_raw(width, height, rgba_data_for_resize)
        .ok_or("Failed to create image buffer")?;

    // Resize to max 1280px width for faster processing
    // Using Triangle filter for maximum quality on high-res displays
    let max_width = 1280u32;
    let resized = if rgba_image.width() > max_width {
        let scale = max_width as f32 / rgba_image.width() as f32;
        let new_height = (rgba_image.height() as f32 * scale) as u32;
        image::imageops::resize(
            &rgba_image,
            max_width,
            new_height,
            image::imageops::FilterType::Triangle,
        )
    } else {
        rgba_image
    };

    // Convert to JPEG (much faster than PNG, smaller size)
    let mut jpeg_data: Vec<u8> = Vec::new();
    let mut cursor = Cursor::new(&mut jpeg_data);

    // Use a slightly lower quality for faster encoding and smaller payload
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut cursor, 75);
    let dynamic_image = image::DynamicImage::ImageRgba8(resized);

    encoder
        .encode_image(&dynamic_image.to_rgb8())
        .map_err(|e| format!("Failed to encode JPEG: {}", e))?;

    // DEBUG: Save resized image for inspection (only when env var is set)
    if std::env::var(SHARD_DEBUG_SCREENSHOTS_ENV).is_ok() {
        log::warn!("[ScreenContext] DEBUG: Writing resized screenshot to disk");
        let resized_jpeg = jpeg_data.clone();
        std::thread::spawn(move || {
            if let Some(cache_dir) = dirs::cache_dir() {
                let debug_dir = cache_dir.join("shard").join("debug");
                let _ = std::fs::create_dir_all(&debug_dir);
                let debug_path = debug_dir.join("shard_screen_debug.jpg");
                let _ = std::fs::write(debug_path, resized_jpeg);
            }
        });
    }

    let base64_data = base64::engine::general_purpose::STANDARD.encode(&jpeg_data);

    // Return FULL RESOLUTION image for OCR (using the original copy)
    let full_res_rgb = image::RgbaImage::from_raw(width, height, rgba_data_for_ocr)
        .ok_or("Failed to recreate full res image")?;
    let full_res_rgb = image::DynamicImage::ImageRgba8(full_res_rgb).to_rgb8();

    Ok((base64_data, "image/jpeg".to_string(), full_res_rgb))
}

/// Check if we should capture based on debounce timing
pub fn should_capture() -> bool {
    // Use unwrap_or_else to recover from poisoned mutex
    let cache = CONTEXT_CACHE.lock().unwrap_or_else(|e| {
        log::warn!("[ScreenContext] Cache mutex was poisoned, recovering");
        e.into_inner()
    });
    match cache.last_capture {
        None => true,
        Some(last) => last.elapsed().as_millis() as u64 >= CAPTURE_DEBOUNCE_MS,
    }
}

/// Get cached context if still valid
pub fn get_cached_context() -> Option<ScreenContext> {
    let cache = CONTEXT_CACHE.lock().unwrap_or_else(|e| {
        log::warn!("[ScreenContext] Cache mutex was poisoned, recovering");
        e.into_inner()
    });
    if let (Some(last), Some(ctx)) = (&cache.last_capture, &cache.last_context) {
        if (last.elapsed().as_millis() as u64) < CACHE_TTL_MS {
            return Some(ctx.clone());
        }
    }
    None
}

/// Capture screen and analyze with Vision LLM
pub async fn capture_and_analyze(
    agent: &crate::agent::Agent,
    config: &AppConfig,
) -> Result<ScreenContext, String> {
    // Check cache first
    if let Some(cached) = get_cached_context() {
        log::info!("[ScreenContext] Returning cached context");
        return Ok(cached);
    }

    // Check debounce
    if !should_capture() {
        return Err("Capture debounce active, skipping".to_string());
    }

    // Update last capture time
    {
        let mut cache = CONTEXT_CACHE.lock().unwrap_or_else(|e| {
            log::warn!("[ScreenContext] Cache mutex was poisoned, recovering");
            e.into_inner()
        });
        cache.last_capture = Some(Instant::now());
    }

    let capture_start = Instant::now();
    log::info!("[ScreenContext] Capturing screen...");
    let (image_base64, mime_type, rgb_image) = capture_screen()?;
    let capture_elapsed = capture_start.elapsed();
    log::info!("[ScreenContext] Screen capture took {:?}", capture_elapsed);

    // Step 1: Local OCR with ocrs (with fallback on failure)
    let ocr_start = Instant::now();
    log::info!("[ScreenContext] Running local OCR...");

    let local_ocr_text = match get_ocr_engine(&agent.app_handle).await {
        Ok(ocr_engine) => {
            // Use FULL resolution image for OCR (as requested)
            // ocr-rs 2.0.2 expects a &DynamicImage
            let dynamic_image = image::DynamicImage::ImageRgb8(rgb_image);
            match ocr_engine.recognize(&dynamic_image) {
                Ok(results) => {
                    results
                        .iter()
                        .map(|r| r.text.clone())
                        .collect::<Vec<String>>()
                        .join("\n")
                }
                Err(e) => {
                    log::warn!("[ScreenContext] OCR recognition failed, continuing without OCR: {}", e);
                    String::new()
                }
            }
        }
        Err(e) => {
            log::warn!("[ScreenContext] Failed to load OCR engine, continuing without OCR: {}", e);
            String::new()
        }
    };

    let ocr_elapsed = ocr_start.elapsed();
    log::info!("[ScreenContext] OCR took {:?}, extracted {} chars", ocr_elapsed, local_ocr_text.len());

    log::info!(
        "[ScreenContext] Local OCR Result: {} chars",
        local_ocr_text.len()
    );

    // Step 2: Analyze with Vision LLM (Image + Local OCR Context)
    let vision_start = Instant::now();
    log::info!("[ScreenContext] Analyzing with Vision LLM (Image + Local OCR)...");

    let enriched_prompt = format!(
        "{}\n\nCONTEXT FROM LOCAL OCR:\n{}",
        SCREEN_CONTEXT_PROMPT, local_ocr_text
    );

    let http_client = reqwest::Client::new();
    let analysis = vision_llm::process_image_with_context(
        &http_client,
        &image_base64,
        &mime_type,
        &enriched_prompt,
        config,
    )
    .await?;

    let vision_elapsed = vision_start.elapsed();
    log::info!("[ScreenContext] Vision LLM analysis took {:?}", vision_elapsed);

    // Parse JSON response (with fallback for malformed responses)
    let (summary, suggestions) = parse_analysis_response(&analysis)?;

    let context = ScreenContext {
        capture_time_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64,
        suggestions,
        context_summary: summary,
        image_base64,
        mime_type,
        ocr_text: local_ocr_text,
    };

    // Cache the result
    {
        let mut cache = CONTEXT_CACHE.lock().unwrap_or_else(|e| {
            log::warn!("[ScreenContext] Cache mutex was poisoned, recovering");
            e.into_inner()
        });
        cache.last_context = Some(context.clone());
    }

    // Log total timing
    let total_elapsed = capture_start.elapsed();
    log::info!(
        "[ScreenContext] Total pipeline: {:?} (capture: {:?}, OCR: {:?}, vision: {:?})",
        total_elapsed, capture_elapsed, ocr_elapsed, vision_elapsed
    );

    log::info!(
        "[ScreenContext] Generated {} suggestions",
        context.suggestions.len()
    );
    Ok(context)
}

/// Parse Vision LLM response into (summary, suggestions)
/// Handles edge cases: markdown wrapping, missing fields, empty arrays
fn parse_analysis_response(response: &str) -> Result<(String, Vec<String>), String> {
    // Try to extract JSON from response (may be wrapped in markdown code block)
    let json_str = response
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```JSON")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();

    // Handle empty response
    if json_str.is_empty() {
        log::warn!("[ScreenContext] Empty response from Vision LLM");
        return Ok(("Unable to analyze screen content".to_string(), vec![]));
    }

    #[derive(serde::Deserialize)]
    struct AnalysisResponse {
        #[serde(default)]
        summary: Option<String>,
        #[serde(default)]
        suggestions: Vec<String>,
    }

    match serde_json::from_str::<AnalysisResponse>(json_str) {
        Ok(parsed) => {
            let summary = parsed.summary.unwrap_or_else(|| "Screen content analyzed".to_string());
            Ok((summary, parsed.suggestions))
        }
        Err(e) => {
            log::warn!("[ScreenContext] Failed to parse JSON, attempting regex fallback: {}", e);
            // Fallback: try to extract any JSON object from the response
            if let Some(start) = response.find('{') {
                if let Some(end) = response.rfind('}') {
                    let json_substr = &response[start..=end];
                    if let Ok(parsed) = serde_json::from_str::<AnalysisResponse>(json_substr) {
                        let summary = parsed.summary.unwrap_or_else(|| "Screen content analyzed".to_string());
                        return Ok((summary, parsed.suggestions));
                    }
                }
            }
            Err(format!(
                "Failed to parse analysis JSON: {} - Response: {}",
                e, &response[..response.len().min(200)]
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_analysis_response() {
        let json = r#"{"summary": "User browsing web", "suggestions": ["Summarize this page", "Extract key points"]}"#;
        let (summary, suggestions) = parse_analysis_response(json).unwrap();
        assert_eq!(summary, "User browsing web");
        assert_eq!(suggestions.len(), 2);
    }

    #[test]
    fn test_parse_markdown_wrapped_json() {
        let json = "```json\n{\"summary\": \"test\", \"suggestions\": [\"a\"]}\n```";
        let (summary, _suggestions) = parse_analysis_response(json).unwrap();
        assert_eq!(summary, "test");
    }

    #[test]
    fn test_parse_uppercase_json_tag() {
        let json = "```JSON\n{\"summary\": \"test\", \"suggestions\": [\"a\", \"b\"]}\n```";
        let (summary, suggestions) = parse_analysis_response(json).unwrap();
        assert_eq!(summary, "test");
        assert_eq!(suggestions.len(), 2);
    }

    #[test]
    fn test_parse_empty_suggestions() {
        let json = r#"{"summary": "Empty desktop", "suggestions": []}"#;
        let (summary, suggestions) = parse_analysis_response(json).unwrap();
        assert_eq!(summary, "Empty desktop");
        assert!(suggestions.is_empty());
    }

    #[test]
    fn test_parse_missing_summary() {
        let json = r#"{"suggestions": ["Do something"]}"#;
        let (summary, suggestions) = parse_analysis_response(json).unwrap();
        assert_eq!(summary, "Screen content analyzed"); // Default fallback
        assert_eq!(suggestions.len(), 1);
    }

    #[test]
    fn test_parse_empty_response() {
        let (summary, suggestions) = parse_analysis_response("").unwrap();
        assert_eq!(summary, "Unable to analyze screen content");
        assert!(suggestions.is_empty());
    }

    #[test]
    fn test_parse_whitespace_only() {
        let (summary, suggestions) = parse_analysis_response("   \n\t  ").unwrap();
        assert_eq!(summary, "Unable to analyze screen content");
        assert!(suggestions.is_empty());
    }

    #[test]
    fn test_parse_embedded_json() {
        // Vision LLM sometimes adds prose around the JSON
        let response = "Here is my analysis:\n{\"summary\": \"coding\", \"suggestions\": [\"help\"]}\nHope that helps!";
        let (summary, suggestions) = parse_analysis_response(response).unwrap();
        assert_eq!(summary, "coding");
        assert_eq!(suggestions.len(), 1);
    }

    #[test]
    fn test_parse_invalid_json_fails() {
        let result = parse_analysis_response("this is not json");
        assert!(result.is_err());
    }

    #[test]
    fn test_debounce_constants() {
        // Verify constants are reasonable
        assert!(CAPTURE_DEBOUNCE_MS >= 1000, "Debounce should be at least 1s");
        assert!(CACHE_TTL_MS >= CAPTURE_DEBOUNCE_MS, "Cache TTL should be >= debounce");
    }
}
