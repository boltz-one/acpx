//! Shared subprocess-probe helper for the Gemini/Copilot runtime quirks.
//! Ports `readCommandOutput` from `others/acpx/src/acp/agent-command.ts`:
//! spawn `command args...` with stdin closed, collect combined
//! stdout+stderr, and resolve to `None` on spawn error or timeout
//! (SIGKILL-ing the child in the timeout case). This probe never surfaces
//! an error to its caller — a failed/timed-out probe just means "unknown",
//! matching acpx's best-effort capability-detection semantics.

use std::process::Stdio;
use std::time::Duration;

use futures::future::{Either, select};
use futures::pin_mut;
use smol::Timer;
use smol::io::AsyncReadExt;

/// Spawns `command args...` and returns `Some("{stdout}\n{stderr}")` on a
/// clean exit (even with empty output, matching acpx's unconditional
/// `` `${stdout}\n${stderr}` `` join), or `None` if the process fails to
/// spawn or does not finish within `timeout`.
pub async fn read_command_output(
    command: &str,
    args: &[String],
    timeout: Duration,
) -> Option<String> {
    let mut cmd = crate::util::command::new_std_command(command);
    cmd.args(args);

    let mut child =
        crate::util::process::Child::spawn(cmd, Stdio::null(), Stdio::piped(), Stdio::piped())
            .ok()?;

    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();

    let collect = async move {
        let mut out = Vec::new();
        let mut err = Vec::new();
        if let Some(stream) = stdout.as_mut() {
            let _ = stream.read_to_end(&mut out).await;
        }
        if let Some(stream) = stderr.as_mut() {
            let _ = stream.read_to_end(&mut err).await;
        }
        format!(
            "{}\n{}",
            String::from_utf8_lossy(&out),
            String::from_utf8_lossy(&err)
        )
    };
    pin_mut!(collect);
    let timer = Timer::after(timeout);

    match select(collect, timer).await {
        Either::Left((combined, _)) => {
            // Best-effort reap: stdout/stderr are already at EOF by this
            // point, so the process has either already exited or is about
            // to, and this should resolve immediately.
            let _ = child.status().await;
            Some(combined)
        }
        Either::Right((_, _)) => {
            let _ = child.kill();
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn missing_program_returns_none() {
        smol::block_on(async {
            let output = read_command_output(
                "/definitely/not/a/real/binary-xyz",
                &[],
                Duration::from_millis(500),
            )
            .await;
            assert!(output.is_none());
        });
    }

    #[cfg(unix)]
    #[test]
    fn collects_combined_stdout_and_stderr() {
        smol::block_on(async {
            let output = read_command_output(
                "/bin/sh",
                &[
                    "-c".to_string(),
                    "echo out-line; echo err-line 1>&2".to_string(),
                ],
                Duration::from_secs(2),
            )
            .await
            .expect("real subprocess should produce output");
            assert!(output.contains("out-line"));
            assert!(output.contains("err-line"));
        });
    }

    #[cfg(unix)]
    #[test]
    fn timeout_kills_child_and_returns_none_promptly() {
        smol::block_on(async {
            let start = Instant::now();
            let output = read_command_output(
                "/bin/sh",
                &["-c".to_string(), "sleep 5".to_string()],
                Duration::from_millis(50),
            )
            .await;
            assert!(output.is_none());
            assert!(
                start.elapsed() < Duration::from_secs(2),
                "expected the timed-out child to be killed promptly, took {:?}",
                start.elapsed()
            );
        });
    }
}
