//! Ports `others/acpx/src/mcp-servers.ts`.
//!
//! acpx hand-validates each `mcpServers[i]` field because it receives
//! untyped JSON from CLI flags/config files and `agent-client-protocol-sdk`
//! (the TS SDK)'s `McpServer` type is a plain interface with no runtime
//! validation attached. The Rust SDK's `agent_client_protocol_schema::v1::McpServer`
//! is a real `serde`-tagged enum (`Http`/`Sse`/`Stdio`, see ADR-1's "reuse
//! the SDK for everything it covers"), so parsing untrusted JSON into it is
//! exactly what `serde_json::from_value` already validates — re-deriving
//! acpx's manual field checks here would duplicate that. This module is
//! scoped to **passthrough only** (forward parsed config into
//! `session/new`'s `mcpServers` field): no local MCP server hosting or
//! brokering, per the phase's resolved open question.

use agent_client_protocol::schema::v1::McpServer;
use serde_json::Value;

use crate::error::{AcpError, Result};

/// Parses a JSON array (typically a session/runtime config's `mcpServers`
/// field) into typed [`McpServer`] entries. `source_path` is included in
/// error messages purely for diagnostics (e.g. the config file path),
/// mirroring acpx's `parseMcpServers(value, sourcePath)`.
pub fn parse_mcp_servers(value: &Value, source_path: &str) -> Result<Vec<McpServer>> {
    if !value.is_array() {
        return Err(AcpError::Other(anyhow::anyhow!(
            "Invalid mcpServers in {source_path}: expected array"
        )));
    }
    serde_json::from_value(value.clone()).map_err(|source| {
        AcpError::Other(anyhow::anyhow!(
            "Invalid mcpServers in {source_path}: {source}"
        ))
    })
}

/// Ports `parseOptionalMcpServers`: `None` short-circuits without error;
/// `Some` is validated via [`parse_mcp_servers`].
pub fn parse_optional_mcp_servers(
    value: Option<&Value>,
    source_path: &str,
) -> Result<Option<Vec<McpServer>>> {
    let Some(value) = value else {
        return Ok(None);
    };
    parse_mcp_servers(value, source_path).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_stdio_server() {
        let value = json!([{
            "name": "fs",
            "command": "npx",
            "args": ["-y", "@modelcontextprotocol/server-filesystem"],
            "env": [{"name": "FOO", "value": "bar"}]
        }]);
        let servers = parse_mcp_servers(&value, "test").unwrap();
        assert_eq!(servers.len(), 1);
        assert!(matches!(servers[0], McpServer::Stdio(_)));
    }

    #[test]
    fn parses_http_server() {
        let value = json!([{
            "type": "http",
            "name": "remote",
            "url": "https://example.com/mcp",
            "headers": [{"name": "Authorization", "value": "Bearer x"}]
        }]);
        let servers = parse_mcp_servers(&value, "test").unwrap();
        assert!(matches!(servers[0], McpServer::Http(_)));
    }

    #[test]
    fn rejects_non_array() {
        assert!(parse_mcp_servers(&json!({"not": "an array"}), "test").is_err());
    }

    #[test]
    fn optional_none_short_circuits() {
        assert_eq!(parse_optional_mcp_servers(None, "test").unwrap(), None);
    }
}
