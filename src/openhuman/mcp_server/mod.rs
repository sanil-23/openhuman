//! MCP server for exposing a curated OpenHuman tool surface.
//!
//! Opt-in via `openhuman-core mcp` (stdio) or `openhuman-core mcp --transport http`.
//! Stdio mode writes newline-delimited JSON-RPC to stdout; HTTP mode speaks
//! Streamable HTTP + SSE on a local bind address. Diagnostics go through stderr logging.
//!
//! Most tools (memory tree reads, core/agent introspection) are read-only and
//! gated through `SecurityPolicy` with `ToolOperation::Read`. The one
//! exception is `agent.run_subagent`, which runs through `ToolOperation::Act`
//! and is advertised to clients via MCP tool annotations
//! (`readOnlyHint: false`, `destructiveHint: true`).

mod http;
mod protocol;
mod resources;
mod session;
mod stdio;
mod tools;
mod write_dispatch;

use std::net::SocketAddr;

pub use http::{run_http, run_http_reporting, HttpServerConfig};
pub use stdio::run_stdio_from_cli;
pub use tools::{tool_specs, McpToolSpec};

/// Lazily-started, process-wide in-process HTTP MCP server bound to localhost
/// on an ephemeral port. The Claude Code provider points the sandboxed `claude`
/// subprocess at this URL so it can reach OpenHuman's memory/tools over
/// loopback **without** the MCP server inheriting CC's OS jail — the server
/// runs here, in the trusted (unjailed) core process, with full workspace
/// access, while CC's own raw tools are denied any access to `~/.openhuman`.
static LOCAL_HTTP_ADDR: tokio::sync::OnceCell<SocketAddr> = tokio::sync::OnceCell::const_new();

/// Ensure the in-process HTTP MCP server is running and return its localhost
/// address. Idempotent: the server is started once and reused across turns.
///
/// Auth is omitted (localhost-only bind); add a per-launch bearer token as a
/// hardening follow-up if local-process isolation becomes a concern.
pub async fn ensure_local_http() -> anyhow::Result<SocketAddr> {
    LOCAL_HTTP_ADDR
        .get_or_try_init(|| async {
            let (tx, rx) = tokio::sync::oneshot::channel();
            tokio::spawn(async move {
                let config = HttpServerConfig {
                    bind_addr: "127.0.0.1:0".parse().expect("valid loopback addr"),
                    auth_token: None,
                };
                if let Err(e) = run_http_reporting(config, Some(tx)).await {
                    log::error!("[mcp_server] in-process HTTP MCP server exited: {e}");
                }
            });
            let addr = rx
                .await
                .map_err(|_| anyhow::anyhow!("MCP HTTP server never reported its bind address"))?;
            log::info!("[mcp_server] in-process HTTP MCP server ready on {addr}");
            Ok::<SocketAddr, anyhow::Error>(addr)
        })
        .await
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn ensure_local_http_binds_loopback_and_is_idempotent() {
        let a = ensure_local_http().await.expect("first start");
        assert!(a.ip().is_loopback(), "must bind loopback only, got {a}");
        assert_ne!(a.port(), 0, "must report a concrete bound port");
        // Singleton: a second call returns the same address, not a new server.
        let b = ensure_local_http().await.expect("second start");
        assert_eq!(a, b, "ensure_local_http must be a process-wide singleton");
    }
}
