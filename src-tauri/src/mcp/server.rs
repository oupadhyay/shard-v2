//! Phase 3.3 — MCP server (`ServerHandler` over stdio).
//!
//! Implements the curated tool surface from [`super::handlers`] as an
//! `rmcp::ServerHandler`. The `run_stdio_server()` entry point wires the
//! server to `(tokio::io::stdin(), tokio::io::stdout())` so the binary
//! launched as `shard --mcp` speaks MCP on stdio out of the box.
//!
//! Tool routing is dynamic (rather than `#[tool_router]` macro-based)
//! because Shard's tool surface lives in the global registry; we want
//! the MCP server to query that registry rather than redeclare schemas.

use std::borrow::Cow;
use std::future::Future;
use std::sync::Arc;

use rmcp::{
    handler::server::ServerHandler,
    model::{
        CallToolRequestParam, CallToolResult, Content, ErrorData as McpError,
        Implementation, InitializeResult, ListToolsResult, PaginatedRequestParam,
        ProtocolVersion, ServerCapabilities, ServerInfo, Tool,
    },
    service::{RequestContext, RoleServer, ServiceExt},
};
use serde_json::Value;

use super::handlers::{
    handle_action_next, handle_action_plan, handle_edit_file, handle_file_history,
    handle_memory_search, handle_read_file, handle_save_memory, CURATED_TOOL_NAMES,
};

/// MCP server handler — pure routing over [`crate::tool_registry`] and
/// [`super::handlers`]. Cloneable so `rmcp::serve_server` can pass it
/// across the spawned reader / writer tasks.
#[derive(Clone)]
pub struct ShardMcpServer {
    /// Serializes the curated-subset write tools (`edit_file`,
    /// `save_memory`, `action_plan`). Multiple stdio clients are
    /// uncommon but supported; the mutex enforces a strict last-write-
    /// wins ordering when they collide.
    write_lock: Arc<tokio::sync::Mutex<()>>,
}

impl Default for ShardMcpServer {
    fn default() -> Self {
        Self::new()
    }
}

impl ShardMcpServer {
    pub fn new() -> Self {
        Self {
            write_lock: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    /// Build the MCP `Tool` descriptor for one of our curated tools by
    /// looking up its JSON schema in [`crate::tool_registry::global`].
    fn tool_descriptor(name: &'static str) -> Option<Tool> {
        let entry = crate::tool_registry::global().get(name)?;
        let params = entry.schema.function.parameters.clone();
        let obj = params.as_object().cloned().unwrap_or_default();
        Some(Tool::new(
            Cow::Borrowed(name),
            Cow::Owned(entry.schema.function.description.clone()),
            Arc::new(obj),
        ))
    }

    /// Curated list assembled from [`CURATED_TOOL_NAMES`].
    pub fn list_curated_tools() -> Vec<Tool> {
        CURATED_TOOL_NAMES
            .iter()
            .filter_map(|n| Self::tool_descriptor(n))
            .collect()
    }

    /// Test-only accessor for the per-server write mutex so unit tests
    /// can exercise the concurrent-edit serialisation contract without
    /// going through the full rmcp transport. Marked `#[doc(hidden)]`
    /// because outside callers should always go through `call_tool`.
    #[doc(hidden)]
    pub fn write_lock_for_test(&self) -> Arc<tokio::sync::Mutex<()>> {
        self.write_lock.clone()
    }

    /// Synchronous dispatch — `tools/call` for one of the curated names.
    /// Returns either the tool's success string or an MCP error.
    async fn dispatch(&self, name: &str, args: Value) -> Result<String, McpError> {
        // Coarse write-side mutex so concurrent stdio clients can't
        // tear allow-listed file edits.
        let _guard = match name {
            "edit_file" | "save_memory" | "action_plan" => Some(self.write_lock.lock().await),
            _ => None,
        };
        let result = match name {
            "memory_search" => handle_memory_search(&args),
            "save_memory" => handle_save_memory(&args),
            "file_history" => handle_file_history(&args),
            "read_file" => handle_read_file(&args),
            "edit_file" => handle_edit_file(&args),
            "action_next" => handle_action_next(&args),
            "action_plan" => handle_action_plan(&args),
            other => {
                return Err(McpError::invalid_request(
                    format!("`{}` is not exposed over MCP", other),
                    None,
                ));
            }
        };
        drop(_guard);
        result.map_err(|e| McpError::internal_error(e, None))
    }
}

impl ServerHandler for ShardMcpServer {
    fn get_info(&self) -> ServerInfo {
        let mut caps = ServerCapabilities::default();
        // Advertise the tools capability — that's what MCP clients look
        // for before calling `tools/list`.
        caps.tools = Some(rmcp::model::ToolsCapability {
            list_changed: Some(false),
        });
        InitializeResult {
            protocol_version: ProtocolVersion::default(),
            capabilities: caps,
            server_info: Implementation {
                name: "shard-mcp".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
            instructions: Some(
                "Shard's memory + self-edit primitives over MCP. Curated, read-mostly subset of the agent toolset."
                    .to_string(),
            ),
        }
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, McpError>> + Send + '_ {
        async move {
            Ok(ListToolsResult {
                tools: Self::list_curated_tools(),
                next_cursor: None,
            })
        }
    }

    fn call_tool(
        &self,
        request: CallToolRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<CallToolResult, McpError>> + Send + '_ {
        async move {
            let args = request
                .arguments
                .map(Value::Object)
                .unwrap_or(Value::Null);
            match self.dispatch(&request.name, args).await {
                Ok(text) => Ok(CallToolResult::success(vec![Content::text(text)])),
                Err(e) => {
                    let msg = e.to_string();
                    Ok(CallToolResult::error(vec![Content::text(msg)]))
                }
            }
        }
    }
}

/// Entry point: serve the MCP protocol on stdin/stdout. Returns when the
/// peer disconnects.
pub async fn run_stdio_server() -> Result<(), String> {
    let server = ShardMcpServer::new();
    let (stdin, stdout) = rmcp::transport::io::stdio();
    let running = server
        .serve((stdin, stdout))
        .await
        .map_err(|e| format!("MCP server failed to start: {}", e))?;
    running
        .waiting()
        .await
        .map_err(|e| format!("MCP server loop ended with error: {}", e))?;
    Ok(())
}
