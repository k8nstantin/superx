/*
 * SuperX MCP Server (library surface) — Revision 42.14
 *
 * Copyright (c) 2026 Constantin Alexander <constantin@dedomena.io>
 *
 * The MCP dispatch policy is extracted here so it can be exercised directly in
 * integration tests without standing up a full rmcp transport / RequestContext.
 * `McpServer::call_tool` is the thinnest possible delegate to `dispatch_tool`.
 */

#![deny(warnings)]
#![deny(clippy::pedantic)]

use rmcp::model::{
    CallToolRequestMethod, CallToolRequestParams, CallToolResult, Content,
    ListToolsResult, PaginatedRequestParams, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler};
use serde_json::{json, Map, Value};
use std::sync::Arc;
use superx_agent::CapabilityGovernor;
use superx_compiler::CompilerBlade;
use superx_ingest::{FileSource, UniversalIngestor};
use superx_kernel::Kernel;

pub struct McpServer {
    pub kernel: Arc<Kernel>,
}

impl McpServer {
    #[must_use]
    pub fn new(kernel: Arc<Kernel>) -> Self {
        Self { kernel }
    }
}

/// Default tenant fallback used when an MCP request does not carry one.
/// Re-exported from `superx-kernel` so there's exactly one source of truth.
pub use superx_kernel::DEFAULT_TENANT;

/// Dispatch an MCP tool call. Pure policy: takes the kernel + the parsed request
/// pieces, returns the same `Result<CallToolResult, McpError>` the rmcp handler
/// would return. Callable directly from tests; no `RequestContext` needed.
///
/// # Errors
/// Returns `McpError::internal_error` on auth / capability failures and
/// `McpError::invalid_params` when required arguments are missing.
pub async fn dispatch_tool(
    kernel: &Kernel,
    name: &str,
    arguments: Option<Map<String, Value>>,
) -> Result<CallToolResult, McpError> {
    let args = arguments.unwrap_or_default();
    let tenant = args
        .get("tenant")
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_TENANT)
        .to_string();

    kernel
        .set_session_auth(&tenant, "user")
        .await
        .map_err(|e| McpError::internal_error(format!("Auth failed: {e}"), None))?;

    match name {
        "identify" => {
            let agent_uid = args
                .get("agent_uid")
                .and_then(Value::as_str)
                .ok_or_else(|| McpError::invalid_params("Missing agent_uid", None))?;
            let gov = CapabilityGovernor::new(kernel);
            match gov.handshake(agent_uid).await {
                Ok(session_id) => Ok(CallToolResult::success(vec![Content::text(
                    json!({"session_id": session_id, "status": "authenticated"}).to_string(),
                )])),
                Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
            }
        }
        "graphify" => {
            let agent_id = args
                .get("agent_id")
                .and_then(Value::as_str)
                .ok_or_else(|| McpError::invalid_params("Missing agent_id", None))?;
            let path = args
                .get("path")
                .and_then(Value::as_str)
                .ok_or_else(|| McpError::invalid_params("Missing path", None))?;

            let gov = CapabilityGovernor::new(kernel);
            gov.check_capability(agent_id, "tool_ingest")
                .await
                .map_err(|e| McpError::internal_error(e.to_string(), None))?;

            let ingestor = UniversalIngestor::new(kernel);
            let run_id = uuid::Uuid::now_v7().to_string();
            let source = Box::new(FileSource { path: path.to_string() });
            match ingestor.ingest(source, &run_id).await {
                Ok(id) => Ok(CallToolResult::success(vec![Content::text(
                    json!({"root_id": id, "run_id": run_id}).to_string(),
                )])),
                Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
            }
        }
        "compile" => {
            let agent_id = args
                .get("agent_id")
                .and_then(Value::as_str)
                .ok_or_else(|| McpError::invalid_params("Missing agent_id", None))?;
            let root_id = args
                .get("root_id")
                .and_then(Value::as_str)
                .ok_or_else(|| McpError::invalid_params("Missing root_id", None))?;

            let gov = CapabilityGovernor::new(kernel);
            gov.check_capability(agent_id, "tool_compile")
                .await
                .map_err(|e| McpError::internal_error(e.to_string(), None))?;

            let tiers = args.get("tiers").and_then(Value::as_array).map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect::<Vec<String>>()
            });
            let compiler = CompilerBlade::new(kernel, None);
            let run_id = uuid::Uuid::now_v7().to_string();
            match compiler.compile(root_id, &run_id, tiers).await {
                Ok(xml) => Ok(CallToolResult::success(vec![Content::text(xml)])),
                Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
            }
        }
        _ => Err(McpError::method_not_found::<CallToolRequestMethod>()),
    }
}

/// MCP tool descriptors advertised by the `SuperX` server. Kept here so tests
/// can inspect the schema without standing up an `rmcp` transport.
///
/// # Panics
/// Panics only if the `json!(...)` literals below are malformed — which would be
/// a compile-time visible mistake, not a runtime condition.
#[must_use]
pub fn list_tools_payload() -> Vec<Tool> {
    vec![
        Tool::new(
            "identify",
            "Identify the agent and start a session",
            json!({
                "type": "object",
                "properties": {
                    "agent_uid": { "type": "string" },
                    "tenant": { "type": "string" }
                },
                "required": ["agent_uid"]
            })
            .as_object()
            .unwrap()
            .clone(),
        ),
        Tool::new(
            "graphify",
            "Ingest a source into SuperX",
            json!({
                "type": "object",
                "properties": {
                    "agent_id": { "type": "string" },
                    "path": { "type": "string" },
                    "tenant": { "type": "string" }
                },
                "required": ["agent_id", "path"]
            })
            .as_object()
            .unwrap()
            .clone(),
        ),
        Tool::new(
            "compile",
            "Distill context from SuperX",
            json!({
                "type": "object",
                "properties": {
                    "agent_id": { "type": "string" },
                    "root_id": { "type": "string" },
                    "tenant": { "type": "string" },
                    "tiers": { "type": "array", "items": { "type": "string" } }
                },
                "required": ["agent_id", "root_id"]
            })
            .as_object()
            .unwrap()
            .clone(),
        ),
    ]
}

impl ServerHandler for McpServer {
    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult {
            tools: list_tools_payload(),
            next_cursor: None,
            meta: None,
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        dispatch_tool(&self.kernel, request.name.as_ref(), request.arguments).await
    }
}
