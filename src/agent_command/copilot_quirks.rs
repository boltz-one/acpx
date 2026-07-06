//! GitHub Copilot CLI ACP runtime quirk ported from
//! `others/acpx/src/acp/agent-command.ts`'s `ensureCopilotAcpSupport`/
//! `buildCopilotAcpUnsupportedMessage`: a `--help` capability probe run
//! before spawning `copilot --acp`, since older Copilot CLI releases don't
//! support ACP stdio mode at all.
//!
//! Deviation from acpx: the TS source re-probes `--help` a second time
//! inside `buildCopilotAcpUnsupportedMessage` even though the caller just
//! fetched it. This port instead threads the already-fetched probe output
//! through, avoiding a redundant subprocess spawn — the message content is
//! identical either way, since both probes observe the same "missing
//! --acp" outcome.

use std::time::Duration;

use super::command_probe::read_command_output;
use crate::error::{AcpError, Result};

const COPILOT_HELP_TIMEOUT_MS: u64 = 2_000;

/// Ports `ensureCopilotAcpSupport`: probes `<command> --help` and fails
/// with [`AcpError::CopilotAcpUnsupported`] only when the probe
/// *successfully* returns output missing `--acp`. A failed or timed-out
/// probe is not itself treated as unsupported (matches acpx's
/// `typeof helpOutput === "string" && !helpOutput.includes(...)` check —
/// an inconclusive probe optimistically lets the spawn proceed).
pub async fn ensure_copilot_acp_support(command: &str) -> Result<()> {
    let help_output = read_command_output(
        command,
        &["--help".to_string()],
        Duration::from_millis(COPILOT_HELP_TIMEOUT_MS),
    )
    .await;

    let supported = help_output
        .as_deref()
        .map(|output| output.contains("--acp"))
        .unwrap_or(true);

    if supported {
        return Ok(());
    }

    Err(AcpError::CopilotAcpUnsupported(
        build_copilot_acp_unsupported_message(help_output.as_deref()),
    ))
}

/// Ports `buildCopilotAcpUnsupportedMessage`, given the `--help` output
/// already fetched by [`ensure_copilot_acp_support`] (see module docs for
/// why this differs from acpx's independent re-probe).
fn build_copilot_acp_unsupported_message(help_output: Option<&str>) -> String {
    let mut parts = vec![
        "GitHub Copilot CLI ACP stdio mode is not available in the installed copilot binary."
            .to_string(),
        "This runtime expects a Copilot CLI release that supports --acp --stdio.".to_string(),
    ];

    if let Some(output) = help_output {
        if !output.contains("--acp") {
            parts.push("Detected copilot --help output without --acp support.".to_string());
        }
    }

    parts.push(
        "Upgrade GitHub Copilot CLI to a release with ACP stdio support, or configure a different ACP-compatible agent command in the meantime."
            .to_string(),
    );
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_binary_is_treated_as_inconclusive_and_allowed() {
        smol::block_on(async {
            let result = ensure_copilot_acp_support("/definitely/not/a/real/binary-xyz").await;
            assert!(result.is_ok());
        });
    }

    #[cfg(unix)]
    #[test]
    fn help_output_with_acp_flag_is_supported() {
        smol::block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let script = write_fake_copilot(&dir, "usage: copilot [--acp] [--stdio]\n");

            let result = ensure_copilot_acp_support(script.to_str().unwrap()).await;
            assert!(result.is_ok());
        });
    }

    #[cfg(unix)]
    #[test]
    fn help_output_without_acp_flag_is_unsupported() {
        smol::block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let script = write_fake_copilot(&dir, "usage: copilot [--chat]\n");

            let err = ensure_copilot_acp_support(script.to_str().unwrap())
                .await
                .expect_err("missing --acp support should error");
            match err {
                AcpError::CopilotAcpUnsupported(message) => {
                    assert!(message.contains("--acp"));
                    assert!(message.contains("Detected copilot --help output"));
                }
                other => panic!("expected CopilotAcpUnsupported, got {other:?}"),
            }
        });
    }

    #[cfg(unix)]
    fn write_fake_copilot(dir: &tempfile::TempDir, help_output: &str) -> std::path::PathBuf {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;

        let path = dir.path().join("copilot");
        let mut file = std::fs::File::create(&path).unwrap();
        writeln!(file, "#!/bin/sh").unwrap();
        write!(file, "printf '{help_output}'").unwrap();
        drop(file);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }
}
