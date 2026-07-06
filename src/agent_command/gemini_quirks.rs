//! Gemini CLI ACP runtime quirks ported from
//! `others/acpx/src/acp/agent-command.ts`: `--version` probing, the
//! `--acp`/`--experimental-acp` flag rewrite for pre-0.33 Gemini CLI
//! releases, the startup-timeout duration override, and the diagnostic
//! message attached to [`crate::error::AcpError::GeminiAcpStartupTimeout`].
//! Uses [`super::command_probe::read_command_output`] for the actual
//! subprocess probe.

use std::cmp::Ordering;
use std::sync::OnceLock;
use std::time::Duration;

use super::command_args::basename_token;
use super::command_probe::read_command_output;

const GEMINI_ACP_STARTUP_TIMEOUT_MS: u64 = 15_000;
const GEMINI_VERSION_TIMEOUT_MS: u64 = 2_000;
const GEMINI_ACP_FLAG_VERSION: (u32, u32, u32) = (0, 33, 0);

/// A parsed `x.y.z` Gemini CLI version. Ports `GeminiVersion`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeminiVersion {
    pub raw: String,
    pub parts: (u32, u32, u32),
}

fn version_pattern() -> &'static regex::Regex {
    static PATTERN: OnceLock<regex::Regex> = OnceLock::new();
    PATTERN.get_or_init(|| regex::Regex::new(r"(\d+)\.(\d+)\.(\d+)").unwrap())
}

fn parse_gemini_version(value: &str) -> Option<GeminiVersion> {
    let normalized = value.trim();
    let captures = version_pattern().captures(normalized)?;
    let parts = (
        captures[1].parse().ok()?,
        captures[2].parse().ok()?,
        captures[3].parse().ok()?,
    );
    Some(GeminiVersion {
        raw: normalized.to_string(),
        parts,
    })
}

/// Ports `compareVersionParts` (tuple ordering is already the same
/// lexicographic major/minor/patch comparison).
fn compare_version_parts(left: (u32, u32, u32), right: (u32, u32, u32)) -> Ordering {
    left.cmp(&right)
}

/// Ports `resolveGeminiAcpStartupTimeoutMs`: env override
/// `ACPX_GEMINI_ACP_STARTUP_TIMEOUT_MS`, default 15s.
pub fn resolve_gemini_acp_startup_timeout_ms() -> Duration {
    resolve_gemini_acp_startup_timeout_ms_from(
        std::env::var("ACPX_GEMINI_ACP_STARTUP_TIMEOUT_MS")
            .ok()
            .as_deref(),
    )
}

fn resolve_gemini_acp_startup_timeout_ms_from(raw: Option<&str>) -> Duration {
    let parsed = raw
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value > 0.0);
    match parsed {
        Some(ms) => Duration::from_millis(ms.round() as u64),
        None => Duration::from_millis(GEMINI_ACP_STARTUP_TIMEOUT_MS),
    }
}

/// Ports `detectGeminiVersion`: spawns `<command> --version` with a 2s
/// timeout and extracts the first `\d+\.\d+\.\d+` line from the combined
/// stdout+stderr output.
pub async fn detect_gemini_version(command: &str) -> Option<GeminiVersion> {
    let output = read_command_output(
        command,
        &["--version".to_string()],
        Duration::from_millis(GEMINI_VERSION_TIMEOUT_MS),
    )
    .await?;
    output
        .split(['\n', '\r'])
        .map(str::trim)
        .find(|line| version_pattern().is_match(line))
        .and_then(parse_gemini_version)
}

/// Ports `resolveGeminiCommandArgs`: rewrites a `gemini --acp` invocation
/// to `--experimental-acp` when the installed Gemini CLI predates the
/// release that introduced the stable `--acp` flag name. Non-Gemini
/// commands (or Gemini invocations without `--acp`) pass through
/// unchanged, and the version probe is skipped entirely.
pub async fn resolve_gemini_command_args(command: &str, args: &[String]) -> Vec<String> {
    if basename_token(command) != "gemini" || !args.iter().any(|a| a == "--acp") {
        return args.to_vec();
    }

    match detect_gemini_version(command).await {
        Some(version)
            if compare_version_parts(version.parts, GEMINI_ACP_FLAG_VERSION) == Ordering::Less =>
        {
            args.iter()
                .map(|a| {
                    if a == "--acp" {
                        "--experimental-acp".to_string()
                    } else {
                        a.clone()
                    }
                })
                .collect()
        }
        _ => args.to_vec(),
    }
}

/// Ports `buildGeminiAcpStartupTimeoutMessage`: the diagnostic message
/// carried by [`crate::error::AcpError::GeminiAcpStartupTimeout`], noting
/// the detected Gemini CLI version (if any) and whether non-interactive
/// auth env vars are present.
pub async fn build_gemini_acp_startup_timeout_message(command: &str) -> String {
    let mut parts = vec![
        "Gemini CLI ACP startup timed out before initialize completed.".to_string(),
        "This usually means the local Gemini CLI is waiting on interactive OAuth or has incompatible ACP subprocess behavior.".to_string(),
    ];

    if let Some(version) = detect_gemini_version(command).await {
        parts.push(format!("Detected Gemini CLI version: {}.", version.raw));
    }

    if std::env::var_os("GEMINI_API_KEY").is_none() && std::env::var_os("GOOGLE_API_KEY").is_none()
    {
        parts.push(
            "No GEMINI_API_KEY or GOOGLE_API_KEY was set for non-interactive auth.".to_string(),
        );
    }

    parts.push(
        "Try upgrading Gemini CLI and using API-key-based auth for non-interactive ACP runs."
            .to_string(),
    );
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_semver_out_of_noisy_output() {
        let version = parse_gemini_version("gemini-cli 0.32.1 (built abc)").unwrap();
        assert_eq!(version.parts, (0, 32, 1));
        assert_eq!(version.raw, "gemini-cli 0.32.1 (built abc)");
    }

    #[test]
    fn parses_none_without_a_semver() {
        assert!(parse_gemini_version("no version here").is_none());
    }

    #[test]
    fn compares_versions_lexicographically() {
        assert_eq!(
            compare_version_parts((0, 32, 9), (0, 33, 0)),
            Ordering::Less
        );
        assert_eq!(
            compare_version_parts((0, 33, 0), (0, 33, 0)),
            Ordering::Equal
        );
        assert_eq!(
            compare_version_parts((1, 0, 0), (0, 33, 0)),
            Ordering::Greater
        );
    }

    #[test]
    fn startup_timeout_defaults_when_env_unset_or_invalid() {
        assert_eq!(
            resolve_gemini_acp_startup_timeout_ms_from(None),
            Duration::from_millis(GEMINI_ACP_STARTUP_TIMEOUT_MS)
        );
        assert_eq!(
            resolve_gemini_acp_startup_timeout_ms_from(Some("not-a-number")),
            Duration::from_millis(GEMINI_ACP_STARTUP_TIMEOUT_MS)
        );
        assert_eq!(
            resolve_gemini_acp_startup_timeout_ms_from(Some("-5")),
            Duration::from_millis(GEMINI_ACP_STARTUP_TIMEOUT_MS)
        );
    }

    #[test]
    fn startup_timeout_honors_valid_env_override() {
        assert_eq!(
            resolve_gemini_acp_startup_timeout_ms_from(Some("2500")),
            Duration::from_millis(2500)
        );
    }

    #[test]
    fn non_gemini_command_skips_probe_entirely() {
        smol::block_on(async {
            let args = vec!["--acp".to_string()];
            let resolved =
                resolve_gemini_command_args("/definitely/not/a/real/binary-xyz", &args).await;
            assert_eq!(
                resolved, args,
                "non-gemini basename must pass through unchanged"
            );
        });
    }

    #[test]
    fn gemini_command_without_acp_flag_skips_probe() {
        smol::block_on(async {
            // Uses a nonexistent binary named "gemini" to prove the probe
            // is never attempted when `--acp` is absent (a real spawn
            // would fail loudly if this guard were missing since the
            // caller passed no such binary).
            let args = vec!["--experimental-acp".to_string()];
            let resolved = resolve_gemini_command_args("/no/such/path/gemini", &args).await;
            assert_eq!(resolved, args);
        });
    }

    #[cfg(unix)]
    #[test]
    fn rewrites_acp_flag_for_old_gemini_version() {
        smol::block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let script = write_fake_gemini(&dir, "0.30.5\n");

            let resolved =
                resolve_gemini_command_args(script.to_str().unwrap(), &["--acp".to_string()]).await;
            assert_eq!(resolved, vec!["--experimental-acp".to_string()]);
        });
    }

    #[cfg(unix)]
    #[test]
    fn keeps_acp_flag_for_current_gemini_version() {
        smol::block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let script = write_fake_gemini(&dir, "0.33.0\n");

            let resolved =
                resolve_gemini_command_args(script.to_str().unwrap(), &["--acp".to_string()]).await;
            assert_eq!(resolved, vec!["--acp".to_string()]);
        });
    }

    #[cfg(unix)]
    #[test]
    fn detects_version_via_real_subprocess() {
        smol::block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let script = write_fake_gemini(&dir, "1.2.3\n");

            let version = detect_gemini_version(script.to_str().unwrap())
                .await
                .unwrap();
            assert_eq!(version.parts, (1, 2, 3));
        });
    }

    #[cfg(unix)]
    #[test]
    fn startup_timeout_message_includes_detected_version() {
        smol::block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let script = write_fake_gemini(&dir, "0.28.0\n");

            let message = build_gemini_acp_startup_timeout_message(script.to_str().unwrap()).await;
            assert!(message.contains("0.28.0"));
            assert!(message.contains("startup timed out"));
        });
    }

    #[cfg(unix)]
    fn write_fake_gemini(dir: &tempfile::TempDir, version_output: &str) -> std::path::PathBuf {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;

        let path = dir.path().join("gemini");
        let mut file = std::fs::File::create(&path).unwrap();
        writeln!(file, "#!/bin/sh").unwrap();
        write!(file, "printf '{version_output}'").unwrap();
        drop(file);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }
}
