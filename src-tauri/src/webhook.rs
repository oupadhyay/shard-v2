use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Runtime, Emitter};
use std::net::SocketAddr;

pub struct WebhookState<R: Runtime> {
    app_handle: AppHandle<R>,
}

impl<R: Runtime> Clone for WebhookState<R> {
    fn clone(&self) -> Self {
        Self {
            app_handle: self.app_handle.clone(),
        }
    }
}

pub async fn start_webhook_server<R: Runtime>(app_handle: AppHandle<R>) {
    let state = WebhookState { app_handle };

    let app = Router::new()
        // Health check endpoint
        .route("/health", get(health_check))
        // Generalized async callback endpoint from tools
        .route("/webhook/callback/{id}", post(handle_callback))
        .with_state(state);

    let port = 0; // Use random port to prevent conflicts with Vite dev server
    let addr = SocketAddr::from(([127, 0, 0, 1], port));

    log::info!("[Webhook] Attempting to start local webhook server");

    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(l) => {
            log::info!("[Webhook] Successfully bound to port {}", l.local_addr().unwrap());
            l
        },
        Err(e) => {
            log::error!("[Webhook] Failed to bind: {}", e);
            return;
        }
    };

    if let Err(e) = axum::serve(listener, app).await {
        log::error!("[Webhook] Server error: {}", e);
    }
}

async fn health_check() -> &'static str {
    "Shard Webhook Server is healthy"
}

#[derive(Deserialize, Serialize, Debug)]
pub struct WebhookPayload {
    pub status: String,
    pub result: serde_json::Value,
}

fn sanitize_log_field(input: &str) -> String {
    input.replace(&['\n', '\r'][..], " ")
}

async fn handle_callback<R: Runtime>(
    Path(id): Path<String>,
    State(state): State<WebhookState<R>>,
    Json(payload): Json<WebhookPayload>,
) -> &'static str {
    let sanitized_id = sanitize_log_field(&id);
    let sanitized_status = sanitize_log_field(&payload.status);
    let result_compact = match serde_json::to_string(&payload.result) {
        Ok(s) => s,
        Err(_) => String::from("<invalid json>"),
    };
    log::info!(
        "[Webhook] Received callback for request id={} status={} result={}",
        sanitized_id,
        sanitized_status,
        result_compact
    );

    // In the future, we will route this payload back into the active Shard context (UI or Agent)
    // using the id to look up the pending transaction.
    // For now, emit a simple event to the frontend for visibility
    let _ = state.app_handle.emit("webhook-received", serde_json::json!({
        "id": id,
        "payload": payload
    }));

    "Received"
}
