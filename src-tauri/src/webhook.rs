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

    let port = 1420; // Default local port for Shard
    let addr = SocketAddr::from(([127, 0, 0, 1], port));

    log::info!("[Webhook] Starting local webhook server on {}", addr);

    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            log::error!("[Webhook] Failed to bind to {}: {}", addr, e);
            // Fallback to random port if 1420 is taken
            let random_addr = SocketAddr::from(([127, 0, 0, 1], 0));
            match tokio::net::TcpListener::bind(&random_addr).await {
                Ok(l) => {
                    log::info!("[Webhook] Fallback: Bound to random port {}", l.local_addr().unwrap());
                    l
                },
                Err(e) => {
                    log::error!("[Webhook] Failed to bind to fallback port: {}", e);
                    return;
                }
            }
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

async fn handle_callback<R: Runtime>(
    Path(id): Path<String>,
    State(state): State<WebhookState<R>>,
    Json(payload): Json<WebhookPayload>,
) -> &'static str {
    log::info!("[Webhook] Received callback for request {}: {:?}", id, payload);

    // In the future, we will route this payload back into the active Shard context (UI or Agent)
    // using the id to look up the pending transaction.
    // For now, emit a simple event to the frontend for visibility
    let _ = state.app_handle.emit("webhook-received", serde_json::json!({
        "id": id,
        "payload": payload
    }));

    "Received"
}
