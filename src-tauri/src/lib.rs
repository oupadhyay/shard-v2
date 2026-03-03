use tauri::{AppHandle, Emitter, Manager};

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use tauri_plugin_global_shortcut::{self as tauri_gs, GlobalShortcutExt, Shortcut};

// Stream cancellation system
static CURRENT_STREAM_ID: AtomicU64 = AtomicU64::new(0);
static CANCELLED_STREAM_ID: AtomicU64 = AtomicU64::new(0);

pub mod agent;
mod background;
mod cache;
pub mod compaction;
mod config;
pub mod db;
mod gemini_files;
mod integrations;
mod interactions;
pub mod memories;
mod models;
mod prompts;
pub mod retrieval;
mod secrets;
pub mod sessions;
pub mod skills;
mod tools;
pub mod vector_store;
mod webhook;

#[cfg(test)]
mod tests;

use agent::Agent;
use integrations::screen_context;
use integrations::vision_llm;

// --- State Management ---
pub struct AppState {
    pub agent: Arc<Agent>,
    pub memory_store: Arc<RwLock<Option<memories::MemoryStore>>>,
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
    // Wrap all blocking I/O in spawn_blocking to avoid starving the async executor
    let result = tokio::task::spawn_blocking(move || perform_ocr_capture_blocking())
        .await
        .map_err(|e| {
            if e.is_cancelled() {
                "OCR task was cancelled".to_string()
            } else {
                format!("OCR task panicked: {}", e)
            }
        })??;

    Ok(result)
}

/// macOS: Use native `/usr/sbin/screencapture -i` for interactive region selection
#[cfg(target_os = "macos")]
fn perform_ocr_capture_blocking() -> Result<OcrResult, String> {
    let temp_dir = std::env::temp_dir();
    // Use a unique temp file name to avoid predictable path attacks
    let temp_path = temp_dir.join(format!("shard_ocr_{}.png", uuid::Uuid::new_v4()));

    // Execute screencapture with absolute path, passing Path/OsStr directly
    let output = std::process::Command::new("/usr/sbin/screencapture")
        .arg("-i")
        .arg(&temp_path)
        .output()
        .map_err(|e| format!("Failed to execute screencapture: {}", e))?;

    if !output.status.success() {
        // Clean up any partial file and return a generic cancellation/failure error
        let _ = std::fs::remove_file(&temp_path);
        return Err("Capture cancelled or failed".to_string());
    }

    // Read image and validate it's non-empty (guards against corrupted/incomplete captures)
    let image_data =
        std::fs::read(&temp_path).map_err(|e| format!("Failed to read capture file: {}", e))?;
    if image_data.is_empty() {
        let _ = std::fs::remove_file(&temp_path);
        return Err("Capture produced an empty file".to_string());
    }

    // Clean up temp file
    if let Err(e) = std::fs::remove_file(&temp_path) {
        log::warn!(
            "Failed to remove temp OCR file {}: {}",
            temp_path.display(),
            e
        );
    }

    // Convert to base64
    let image_base64 =
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &image_data);

    Ok(OcrResult {
        text: "[Processing...]".to_string(),
        image_base64,
        mime_type: "image/png".to_string(),
    })
}

/// Linux/Windows: Use the `screenshots` crate to capture the full primary screen.
/// Interactive region selection is not supported; the full screen is captured instead.
#[cfg(not(target_os = "macos"))]
fn perform_ocr_capture_blocking() -> Result<OcrResult, String> {
    use std::io::Cursor;

    let screens =
        screenshots::Screen::all().map_err(|e| format!("Failed to get screens: {}", e))?;
    let screen = screens.first().ok_or("No screens found")?;

    let captured = screen
        .capture()
        .map_err(|e| format!("Failed to capture screen: {}", e))?;

    // Encode captured RGBA image to PNG in-memory
    let rgba_data = captured.rgba();
    let width = captured.width();
    let height = captured.height();

    let rgba_image = image::RgbaImage::from_raw(width, height, rgba_data.to_vec())
        .ok_or("Failed to create image buffer from capture")?;

    let mut png_buf: Vec<u8> = Vec::new();
    let mut cursor = Cursor::new(&mut png_buf);
    let encoder = image::codecs::png::PngEncoder::new(&mut cursor);
    encoder
        .write_image(
            rgba_image.as_raw(),
            width,
            height,
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|e| format!("Failed to encode PNG: {}", e))?;

    let image_base64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &png_buf);

    Ok(OcrResult {
        text: "[Processing...]".to_string(),
        image_base64,
        mime_type: "image/png".to_string(),
    })
}

// Perform contextual image analysis on a base64-encoded image
// If user_query is provided, analyses image in context of the question
// Otherwise uses a default description prompt
#[tauri::command]
async fn ocr_image(
    app_handle: AppHandle,
    _state: tauri::State<'_, AppState>,
    image_base64: String,
    mime_type: Option<String>,
    user_query: Option<String>,
) -> Result<String, String> {
    // Load config for API keys
    let config = config::load_config(&app_handle)?;

    let mime = mime_type.unwrap_or_else(|| "image/png".to_string());

    // Use the user's query or a default description prompt
    let query = user_query.unwrap_or_else(|| {
        "Describe this image in detail, including any visible text.".to_string()
    });

    // Use contextual Vision LLM for image understanding
    let http_client = reqwest::Client::new();
    vision_llm::process_image_with_context(&http_client, &image_base64, &mime, &query, &config)
        .await
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
    state
        .agent
        .process_message(
            &app_handle,
            message,
            images_base64,
            images_mime_types,
            &config,
            false,
        )
        .await
}

#[tauri::command]
async fn save_and_clear_chat(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let config = crate::config::load_config(&app_handle).map_err(|e| e.to_string())?;
    state.agent.save_and_clear_history(config.gemini_api_key).await;
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
async fn get_chat_history(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<crate::agent::ChatMessage>, String> {
    Ok(state.agent.get_history().await)
}

#[tauri::command]
async fn rewind_history(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.agent.rewind_history().await;
    Ok(())
}

#[tauri::command]
async fn get_recent_sessions(
    app_handle: AppHandle,
    limit: Option<usize>,
) -> Result<String, String> {
    if let Ok(store) = crate::memories::get_vector_store(&app_handle) {
        crate::db::sessions::search_sessions_by_time(&store, "", "all_time", limit.unwrap_or(15))
    } else {
        Err("Failed to open database".to_string())
    }
}

#[tauri::command]
async fn load_session(
    app_handle: AppHandle,
    state: tauri::State<'_, AppState>,
    session_id: String,
) -> Result<(), String> {
    state.agent.load_session_from_db(&app_handle, &session_id).await
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
    state
        .agent
        .retry_with_katex_hint(&app_handle, katex_errors, &config)
        .await
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

/// Open the dedicated (breakout) chat window.
/// If it already exists, bring it to focus.
/// Hides the ambient panel — the frontend handles the fade-out transition
/// before invoking this command.
#[tauri::command]
async fn open_dedicated_window(app_handle: AppHandle) -> Result<(), String> {
    // If the dedicated window already exists, just focus it.
    if let Some(win) = app_handle.get_webview_window("dedicated") {
        win.set_focus().map_err(|e| e.to_string())?;
        return Ok(());
    }

    // Hide the ambient panel (frontend has already faded it out).
    if let Some(main_win) = app_handle.get_webview_window("main") {
        main_win.hide().map_err(|e| e.to_string())?;
    }

    // Create the dedicated window. It loads the same Vite dev server / dist
    // but at `dedicated.html`, which uses a separate entry point with the
    // full session-sidebar layout.
    tauri::WebviewWindowBuilder::new(
        &app_handle,
        "dedicated",
        tauri::WebviewUrl::App("dedicated.html".into()),
    )
    .title("Shard")
    .inner_size(900.0, 660.0)
    .min_inner_size(640.0, 480.0)
    .resizable(true)
    .decorations(false)   // Custom titlebar rendered in HTML
    .transparent(true)
    .shadow(true)
    .center()
    .build()
    .map_err(|e| e.to_string())?;

    Ok(())
}

/// Delete a session and all its messages. If the session is currently active,
/// rotates to a new session so the agent isn't left in a ghost state.
#[tauri::command]
async fn delete_session(
    session_id: String,
    state: tauri::State<'_, AppState>,
    app_handle: AppHandle,
) -> Result<(), String> {
    // Rotate agent state if deleting the active session
    {
        let current = state.agent.session_id.lock().await.clone();
        if current == session_id {
            drop(current);
            let new_id = state.agent.reset_for_delete().await;
            // Create the new session row so FK constraints pass for future messages
            if let Ok(store) = crate::memories::get_vector_store(&app_handle) {
                let now = chrono::Utc::now().to_rfc3339();
                let session = crate::db::sessions::SessionRow {
                    id: new_id,
                    title: "Active Session".to_string(),
                    summary: None,
                    created_at: now.clone(),
                    updated_at: now,
                    active_skills: Some("[]".to_string()),
                };
                let _ = crate::db::sessions::insert_session(&store, &session);
            }
        }
    }

    // Delete messages then session from DB
    if let Ok(store) = crate::memories::get_vector_store(&app_handle) {
        store
            .conn
            .execute(
                "DELETE FROM messages WHERE session_id = ?",
                rusqlite::params![&session_id],
            )
            .map_err(|e| e.to_string())?;
        store
            .conn
            .execute(
                "DELETE FROM sessions WHERE id = ?",
                rusqlite::params![&session_id],
            )
            .map_err(|e| e.to_string())?;
    }

    Ok(())
}

/// Return the session ID currently held by the agent (used by frontend to detect active session).
#[tauri::command]
async fn get_current_session_id(state: tauri::State<'_, AppState>) -> Result<String, String> {
    Ok(state.agent.session_id.lock().await.clone())
}

#[tauri::command]
async fn close_dedicated_window(app_handle: AppHandle) -> Result<(), String> {
    if let Some(win) = app_handle.get_webview_window("dedicated") {
        win.close().map_err(|e| e.to_string())?;
    }
    // Restore the ambient panel and trigger its fade-in via the start-show event.
    if let Some(main_win) = app_handle.get_webview_window("main") {
        main_win.show().map_err(|e| e.to_string())?;
        main_win.set_focus().map_err(|e| e.to_string())?;
        main_win.emit("start-show", ()).ok();
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
fn rebuild_topic_index(app_handle: AppHandle) -> Result<usize, String> {
    // TopicIndex no longer stores embeddings, just file names
    memories::rebuild_topic_index(&app_handle)
}

#[tauri::command]
fn rebuild_insight_index(app_handle: AppHandle) -> Result<usize, String> {
    // InsightIndex no longer stores embeddings, just metadata
    memories::rebuild_insight_index(&app_handle)
}

#[tauri::command]
async fn rebuild_bm25_index(app_handle: AppHandle) -> Result<usize, String> {
    retrieval::rebuild_bm25_index(&app_handle)
}

#[tauri::command]
async fn rebuild_chunk_index(app_handle: AppHandle) -> Result<usize, String> {
    let config = config::load_config(&app_handle)?;
    let api_key = config
        .gemini_api_key
        .ok_or("No Gemini API key configured for embedding generation")?;
    let http_client = reqwest::Client::new();
    memories::rebuild_chunk_index(&app_handle, &http_client, &api_key).await
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

    let context = screen_context::capture_and_analyze(&state.agent, &config).await?;

    // Emit event to frontend
    app_handle.emit("screen-context-ready", &context).ok();

    Ok(context)
}

// --- Main Run Function ---

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(
            tauri_plugin_log::Builder::default()
                .level(log::LevelFilter::Info)
                .filter(|metadata| {
                    !metadata.target().starts_with("html5ever")
                        && !metadata.target().starts_with("selectors")
                })
                .build(),
        );

    #[cfg(target_os = "macos")]
    let builder = builder.plugin(tauri_nspanel::init());

    builder
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .setup(|app| {
            let _app_handle = app.handle();

            // Start background jobs
            background::start_background_jobs(app.handle().clone());

            let webhook_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                crate::webhook::start_webhook_server(webhook_handle).await;
            });

            if let Ok(store) = memories::get_vector_store(&app.handle().clone()) {
                if let Err(e) = crate::db::sessions::run_migration(&app.handle().clone(), &store) {
                    log::warn!("[Startup] Session migration failed: {}", e);
                }
            }

            let agent = Arc::new(Agent::new(app.handle().clone()));
            // Initialize memory store cache
            let memory_store = Arc::new(RwLock::new(None));
            // Load memories immediately
            if let Ok(store) = memories::load_memories_from_disk(&app.handle().clone()) {
                *memory_store.write().unwrap() = Some(store);
            }

            app.manage(AppState {
                agent,
                memory_store,
            });

            // Auto-rebuild chunk index if missing (Phase 2-3: chunks are authoritative)
            let app_handle_for_chunks = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                // Check if chunk index exists in VectorStore
                if let Ok(store) = memories::get_vector_store(&app_handle_for_chunks) {
                    if let Ok(count) = store.chunk_count() {
                        if count > 0 {
                            log::info!("[Startup] Vector store found with {} chunks", count);
                            return;
                        }
                    }
                }

                // Chunk index missing or empty - check if topics/insights exist
                if let Ok(config) = config::load_config(&app_handle_for_chunks) {
                    if let Some(api_key) = config.gemini_api_key {
                        let http_client = reqwest::Client::new();
                        log::info!("[Startup] Chunk index missing, triggering auto-rebuild...");
                        match memories::rebuild_chunk_index(
                            &app_handle_for_chunks,
                            &http_client,
                            &api_key,
                        )
                        .await
                        {
                            Ok(count) => log::info!(
                                "[Startup] Auto-rebuilt chunk index with {} chunks",
                                count
                            ),
                            Err(e) => {
                                log::warn!("[Startup] Failed to auto-rebuild chunk index: {}", e)
                            }
                        }
                    }
                }
            });

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

                    window
                        .set_position(tauri::Position::Physical(tauri::PhysicalPosition { x, y }))
                        .ok();
                }

                let _panel = window.to_panel().unwrap();
            }

            // Register Global Shortcuts with handlers
            let ctrl_space =
                Shortcut::new(Some(tauri_gs::Modifiers::CONTROL), tauri_gs::Code::Space);
            let ctrl_k = Shortcut::new(Some(tauri_gs::Modifiers::CONTROL), tauri_gs::Code::KeyK);

            // Ctrl+Space: Toggle window visibility
            let window_for_space = app.get_webview_window("main").unwrap();
            let app_handle_for_space = app.handle().clone();
            app.handle()
                .global_shortcut()
                .on_shortcut(ctrl_space, move |_app, _shortcut, event| {
                    if event.state == tauri_gs::ShortcutState::Pressed {
                        // If the dedicated window is open, toggle IT instead of ambient
                        if let Some(dedicated) = app_handle_for_space.get_webview_window("dedicated") {
                            if dedicated.is_visible().unwrap_or(false) {
                                dedicated.hide().ok();
                            } else {
                                dedicated.show().ok();
                                dedicated.set_focus().ok();
                            }
                            return;
                        }

                        // No dedicated window — toggle the ambient panel
                        if window_for_space.is_visible().unwrap_or(false) {
                            // Trigger fade out in frontend
                            window_for_space.emit("start-hide", ()).ok();
                        } else {
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
                                        && !config.incognito_mode.unwrap_or(false)
                                    {
                                        // Get Agent from state
                                        let state = app_handle_clone.state::<AppState>();
                                        match screen_context::capture_and_analyze(
                                            &state.agent,
                                            &config,
                                        )
                                        .await
                                        {
                                            Ok(context) => {
                                                log::info!(
                                                    "[ScreenContext] Captured {} suggestions",
                                                    context.suggestions.len()
                                                );
                                                app_handle_clone
                                                    .emit("screen-context-ready", &context)
                                                    .ok();
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
                })
                .ok();

            // Ctrl+K: Trigger OCR
            let window_for_k = app.get_webview_window("main").unwrap();
            app.handle()
                .global_shortcut()
                .on_shortcut(ctrl_k, move |_app, _shortcut, _event| {
                    window_for_k.show().ok();
                    window_for_k.set_focus().ok();
                    window_for_k.emit("trigger-ocr", ()).ok();
                })
                .ok();

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_config,
            get_available_models,
            save_config,
            perform_ocr_capture,
            ocr_image,
            chat,
            save_and_clear_chat,
            restore_chat,
            get_message_count,
            has_backup,
            get_chat_history,
            get_recent_sessions,
            load_session,
            cancel_current_stream,
            rewind_history,
            hide_window,
            open_dedicated_window,
            close_dedicated_window,
            delete_session,
            get_current_session_id,
            force_cleanup,
            force_summary,
            rebuild_topic_index,
            rebuild_insight_index,
            rebuild_bm25_index,
            rebuild_chunk_index,
            retry_with_katex_hint,
            capture_screen_context
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
