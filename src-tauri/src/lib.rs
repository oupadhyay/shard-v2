use tauri::{AppHandle, Emitter, Manager};

use tauri_plugin_global_shortcut::{
    self as tauri_gs, GlobalShortcutExt, Shortcut,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

// Stream cancellation system
static CURRENT_STREAM_ID: AtomicU64 = AtomicU64::new(0);
static CANCELLED_STREAM_ID: AtomicU64 = AtomicU64::new(0);

mod config;
mod integrations;
mod tools;
mod prompts;
mod agent;
mod gemini_files;
mod memories;
mod interactions;
mod background;
mod cache;
mod models;
pub mod retrieval;

#[cfg(test)]
mod tests;

use integrations::vision_llm;
use integrations::screen_context;
use agent::Agent;

// --- State Management ---
struct AppState {
    agent: Arc<Agent>,
}

// --- Commands ---

#[tauri::command]
async fn get_config(app_handle: AppHandle) -> Result<config::AppConfig, String> {
    config::load_config(&app_handle)
}

/// Response structure for get_available_models command
#[derive(serde::Serialize)]
struct ModelsResponse {
    chat_models: Vec<models::ModelInfo>,
    vision_models: Vec<models::ModelInfo>,
    background_models: Vec<models::ModelInfo>,
}

#[tauri::command]
async fn get_available_models() -> Result<ModelsResponse, String> {
    Ok(ModelsResponse {
        chat_models: models::get_chat_models(),
        vision_models: models::get_vision_models(),
        background_models: models::get_background_models(),
    })
}

#[tauri::command]
async fn save_config(app_handle: AppHandle, config: config::AppConfig) -> Result<(), String> {
    config::save_config(&app_handle, &config)
}

#[derive(serde::Serialize)]
struct OcrResult {
    text: String,
    image_base64: String,
    mime_type: String,
}

#[tauri::command]
async fn perform_ocr_capture(_app_handle: AppHandle) -> Result<OcrResult, String> {
    // Load config for API keys
    // let config = config::load_config(&app_handle)?;

    // Use macOS native screencapture for interactive region selection
    let temp_dir = std::env::temp_dir();
    let temp_path = temp_dir.join("shard_ocr_capture.png");
    let temp_path_str = temp_path.to_string_lossy().to_string();

    // Execute screencapture
    let output = std::process::Command::new("screencapture")
        .arg("-i")
        .arg(&temp_path_str)
        .output()
        .map_err(|e| format!("Failed to execute screencapture: {}", e))?;

    if !output.status.success() {
        if !temp_path.exists() {
            return Err("Capture cancelled or failed".to_string());
        }
    }

    // Read image
    let image_data = std::fs::read(&temp_path)
        .map_err(|e| format!("Failed to read capture file: {}", e))?;

    // Convert to base64
    let image_base64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &image_data);

    // Clean up temp file
    if let Err(e) = std::fs::remove_file(&temp_path) {
        log::warn!("Failed to remove temp OCR file {}: {}", temp_path.display(), e);
    }

    // Return image immediately without waiting for OCR
    // OCR will be triggered by frontend separately
    Ok(OcrResult {
        text: "[Processing...]".to_string(),
        image_base64,
        mime_type: "image/png".to_string(),
    })
}

// Perform OCR on a base64-encoded image (for pasted images)
#[tauri::command]
async fn ocr_image(
    app_handle: AppHandle,
    state: tauri::State<'_, AppState>,
    image_base64: String,
    mime_type: Option<String>,
) -> Result<String, String> {
    // Load config for API keys
    let config = config::load_config(&app_handle)?;

    let mime = mime_type.unwrap_or_else(|| "image/png".to_string());

    // Use Vision LLM for OCR instead of Tesseract
    let http_client = reqwest::Client::new();
    vision_llm::describe_image(&state.agent, &http_client, &image_base64, &mime, &config).await
}

#[tauri::command]
async fn chat(
    app_handle: AppHandle,
    state: tauri::State<'_, AppState>,
    message: String,
    images_base64: Option<Vec<String>>,
    images_mime_types: Option<Vec<String>>,
) -> Result<(), String> {
    let config = config::load_config(&app_handle)?;
    state.agent.process_message(&app_handle, message, images_base64, images_mime_types, &config).await
}

#[tauri::command]
async fn clear_chat(app_handle: tauri::AppHandle, state: tauri::State<'_, AppState>) -> Result<(), String> {
    let config = crate::config::load_config(&app_handle).map_err(|e| e.to_string())?;
    state.agent.clear_history(config.gemini_api_key).await;
    Ok(())
}

#[tauri::command]
async fn save_and_clear_chat(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.agent.save_and_clear_history().await;
    Ok(())
}

#[tauri::command]
async fn restore_chat(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.agent.restore_history().await
}

#[tauri::command]
async fn get_message_count(state: tauri::State<'_, AppState>) -> Result<usize, String> {
    Ok(state.agent.get_message_count().await)
}

#[tauri::command]
async fn has_backup(state: tauri::State<'_, AppState>) -> Result<bool, String> {
    Ok(state.agent.has_backup().await)
}

#[tauri::command]
async fn get_chat_history(state: tauri::State<'_, AppState>) -> Result<Vec<crate::agent::ChatMessage>, String> {
    Ok(state.agent.get_history().await)
}

#[tauri::command]
async fn rewind_history(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.agent.rewind_history().await;
    Ok(())
}

/// Retry the last response with a hint about KaTeX rendering errors
/// Called by frontend when KaTeX parsing fails
#[tauri::command]
async fn retry_with_katex_hint(
    app_handle: AppHandle,
    state: tauri::State<'_, AppState>,
    katex_errors: Vec<String>,
) -> Result<(), String> {
    let config = config::load_config(&app_handle)?;
    state.agent.retry_with_katex_hint(&app_handle, katex_errors, &config).await
}

#[tauri::command]
async fn cancel_current_stream() -> Result<(), String> {
    let current_stream = CURRENT_STREAM_ID.load(Ordering::Relaxed);
    CANCELLED_STREAM_ID.store(current_stream, Ordering::Relaxed);
    Ok(())
}

#[tauri::command]
async fn hide_window(app_handle: AppHandle) -> Result<(), String> {
    if let Some(window) = app_handle.get_webview_window("main") {
        window.hide().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[derive(serde::Serialize)]
struct CleanupResult {
    deleted_count: usize,
    bytes_freed: u64,
    llm_reasoning: Option<String>,
}

#[derive(serde::Serialize)]
struct SummaryStats {
    total_interactions: usize,
    user_messages: usize,
    assistant_messages: usize,
    total_chars: usize,
    topics_updated: Vec<String>,
    llm_reasoning: Option<String>,
}

#[tauri::command]
async fn force_cleanup(app_handle: AppHandle) -> Result<CleanupResult, String> {
    let result = background::force_cleanup(&app_handle).await?;
    Ok(CleanupResult {
        deleted_count: result.deleted_count,
        bytes_freed: result.bytes_freed,
        llm_reasoning: result.llm_reasoning,
    })
}

#[tauri::command]
async fn force_summary(app_handle: AppHandle) -> Result<SummaryStats, String> {
    let result = background::force_summary(&app_handle).await?;
    Ok(SummaryStats {
        total_interactions: result.total_interactions,
        user_messages: result.user_messages,
        assistant_messages: result.assistant_messages,
        total_chars: result.total_chars,
        topics_updated: result.topics_updated,
        llm_reasoning: result.llm_reasoning,
    })
}

#[tauri::command]
async fn rebuild_topic_index(app_handle: AppHandle) -> Result<usize, String> {
    let config = config::load_config(&app_handle)?;
    let api_key = config
        .gemini_api_key
        .ok_or("No Gemini API key configured for embedding generation")?;
    let http_client = reqwest::Client::new();
    memories::rebuild_topic_index(&app_handle, &http_client, &api_key).await
}

#[tauri::command]
async fn rebuild_insight_index(app_handle: AppHandle) -> Result<usize, String> {
    let config = config::load_config(&app_handle)?;
    let api_key = config
        .gemini_api_key
        .ok_or("No Gemini API key configured for embedding generation")?;
    let http_client = reqwest::Client::new();
    memories::rebuild_insight_index(&app_handle, &http_client, &api_key).await
}

#[tauri::command]
async fn rebuild_bm25_index(app_handle: AppHandle) -> Result<usize, String> {
    retrieval::rebuild_bm25_index(&app_handle)
}

/// Capture screen context and return suggestions
/// This is triggered when the window is shown via Ctrl+Space
#[tauri::command]
async fn capture_screen_context(
    app_handle: AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<screen_context::ScreenContext, String> {
    let config = config::load_config(&app_handle)?;

    // Check if feature is enabled and not in incognito mode
    if !config.enable_screen_context.unwrap_or(false) {
        return Err("Screen context disabled".to_string());
    }
    if config.incognito_mode.unwrap_or(false) {
        return Err("Screen context disabled in incognito mode".to_string());
    }

    let http_client = reqwest::Client::new();
    let context = screen_context::capture_and_analyze(&state.agent, &http_client, &config).await?;

    // Emit event to frontend
    app_handle.emit("screen-context-ready", &context).ok();

    Ok(context)
}

// --- Main Run Function ---

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(
            tauri_plugin_log::Builder::default()
                .level(log::LevelFilter::Info)
                .filter(|metadata| {
                    !metadata.target().starts_with("html5ever") && !metadata.target().starts_with("selectors")
                })
                .build()
        )
        .plugin(tauri_nspanel::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .setup(|app| {
            let _app_handle = app.handle();

            // Start background jobs
            background::start_background_jobs(app.handle().clone());

            let agent = Arc::new(Agent::new(app.handle().clone()));
            app.manage(AppState { agent });

            // Setup Panel (macOS)
            #[cfg(target_os = "macos")]
            {
                use tauri_nspanel::WebviewWindowExt;
                let window = app.get_webview_window("main").unwrap();

                // Position window at bottom-left
                if let Some(monitor) = window.current_monitor().ok().flatten() {
                    let screen_size = monitor.size();
                    let window_size = window.outer_size().unwrap();

                    // Position: 20px from left, 20px from bottom
                    let x = 20;
                    let y = screen_size.height as i32 - window_size.height as i32 - 20;

                    window.set_position(tauri::Position::Physical(tauri::PhysicalPosition { x, y })).ok();
                }

                let _panel = window.to_panel().unwrap();
            }

            // Register Global Shortcuts with handlers
            let ctrl_space = Shortcut::new(Some(tauri_gs::Modifiers::CONTROL), tauri_gs::Code::Space);
            let ctrl_k = Shortcut::new(Some(tauri_gs::Modifiers::CONTROL), tauri_gs::Code::KeyK);

            // Ctrl+Space: Toggle window visibility
            let window_for_space = app.get_webview_window("main").unwrap();
            let app_handle_for_space = app.handle().clone();
            app.handle().global_shortcut().on_shortcut(ctrl_space, move |_app, _shortcut, event| {
                if event.state == tauri_gs::ShortcutState::Pressed {
                    if window_for_space.is_visible().unwrap_or(false) {
                        // Trigger fade out in frontend
                        window_for_space.emit("start-hide", ()).ok();
                    } else {
                        // Show window immediately
                        window_for_space.show().ok();
                        window_for_space.set_focus().ok();
                        // Trigger fade in
                        window_for_space.emit("start-show", ()).ok();

                        // Spawn async task to capture screen context (non-blocking)
                        let app_handle_clone = app_handle_for_space.clone();
                        tauri::async_runtime::spawn(async move {
                            // Small delay to let the window fade in first
                            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

                            if let Ok(config) = config::load_config(&app_handle_clone) {
                                if config.enable_screen_context.unwrap_or(false)
                                   && !config.incognito_mode.unwrap_or(false) {
                                    let http_client = reqwest::Client::builder()
                                        .timeout(std::time::Duration::from_secs(30))
                                        .build()
                                        .unwrap_or_else(|_| reqwest::Client::new());

                                    // Get Agent from state
                                    let state = app_handle_clone.state::<AppState>();
                                    match screen_context::capture_and_analyze(&state.agent, &http_client, &config).await {
                                        Ok(context) => {
                                            log::info!("[ScreenContext] Captured {} suggestions", context.suggestions.len());
                                            app_handle_clone.emit("screen-context-ready", &context).ok();
                                        }
                                        Err(e) => {
                                            log::warn!("[ScreenContext] Capture failed: {}", e);
                                        }
                                    }
                                }
                            }
                        });
                    }
                }
            }).ok();

            // Ctrl+K: Trigger OCR
            let window_for_k = app.get_webview_window("main").unwrap();
            app.handle().global_shortcut().on_shortcut(ctrl_k, move |_app, _shortcut, _event| {
                window_for_k.show().ok();
                window_for_k.set_focus().ok();
                window_for_k.emit("trigger-ocr", ()).ok();
            }).ok();

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_config,
            get_available_models,
            save_config,
            perform_ocr_capture,
            ocr_image,
            chat,
            clear_chat,
            save_and_clear_chat,
            restore_chat,
            get_message_count,
            has_backup,
            get_chat_history,
            cancel_current_stream,
            rewind_history,
            hide_window,
            force_cleanup,
            force_summary,
            rebuild_topic_index,
            rebuild_insight_index,
            rebuild_bm25_index,
            retry_with_katex_hint,
            capture_screen_context
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
