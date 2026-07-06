//! ndjson-over-stdio transport. Ports the framing half of
//! `others/acpx/src/acp/client.ts` (`ndJsonStream(...)` wiring).
//!
//! Unlike acpx (which hand-rolls line-buffered ndjson parsing over Node
//! streams because the TS SDK only gives it a raw duplex stream), this
//! crate reuses `agent_client_protocol::ByteStreams`: the Rust SDK is
//! generic over any `futures::{AsyncRead, AsyncWrite}` pair and does its own
//! internal line-buffered ndjson framing (see ADR-1 — reuse the SDK for
//! everything it covers). `smol::process::ChildStdin`/`ChildStdout`
//! implement those same `futures-io` traits directly (smol builds on
//! `futures-lite`), so no `tokio_util::compat` shim (needed in the SDK's own
//! tokio-based examples) is required here.

use crate::util::process::Child;
use agent_client_protocol::ByteStreams;
use smol::process::{ChildStdin, ChildStdout};

use crate::error::{AcpError, Result};

/// The transport type passed to `Client::connect_with`.
pub type AgentByteStreams = ByteStreams<ChildStdin, ChildStdout>;

/// Takes ownership of `child`'s stdin/stdout pipes and wraps them as the
/// ACP transport. Fails if the child wasn't spawned with piped stdio
/// (ports acpx's `requireAgentStdio` guard).
pub fn take_transport(child: &mut Child) -> Result<AgentByteStreams> {
    let stdin = child.stdin.take().ok_or_else(missing_stdio_error)?;
    let stdout = child.stdout.take().ok_or_else(missing_stdio_error)?;
    Ok(ByteStreams::new(stdin, stdout))
}

fn missing_stdio_error() -> AcpError {
    AcpError::Other(anyhow::anyhow!(
        "ACP agent must be spawned with piped stdin/stdout"
    ))
}
